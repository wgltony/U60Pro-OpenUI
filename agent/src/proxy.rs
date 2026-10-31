//! One switch for the proxy backends. At most one of WireGuard and Clash is on; this
//! names the active one and moves between them through each backend's own update, which
//! keeps their lock order, transition guard, rollback and revision checks. Add future
//! backends to `Backend`.
use crate::{clash, wireguard};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Backend {
    Off,
    Wireguard,
    Clash,
    Tailscale,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Switch {
    backend: Backend,
    /// Revision of the backend being turned on, or of the one being turned off.
    expected_revision: u64,
    /// For off: the backend the client shows as on. Revisions are per backend, so after
    /// a switch elsewhere a stale revision could match the other one's counter.
    #[serde(default)]
    active: Option<Backend>,
}

fn entry(id: &str, summary: Result<Value, String>) -> Value {
    let mut v = summary.unwrap_or_else(
        |e| json!({"enabled": false, "revision": 0, "ready": false, "reason": e, "error": null}),
    );
    v["id"] = json!(id);
    v
}
/// Both backends' summaries and the active one. With both saved as on, the Clash
/// supervisor keeps its core stopped, so WireGuard is the one in effect.
fn compose(wireguard: Result<Value, String>, clash: Result<Value, String>) -> Value {
    let backends = [entry("wireguard", wireguard), entry("clash", clash)];
    let on = |i: usize| backends[i]["enabled"] == true;
    let active = if on(0) {
        "wireguard"
    } else if on(1) {
        "clash"
    } else {
        "off"
    };
    json!({"active": active, "conflict": on(0) && on(1), "backends": backends})
}
/// Tailscale mode with an exit node takes over. Without one it only reaches the tailnet:
/// it is active when nothing else is, and otherwise runs `alongside` the active backend.
fn compose_tailscale(mut v: Value, tailscale: Result<Value, String>) -> Value {
    let ts = entry("tailscale", tailscale);
    let alongside = ts["enabled"] == true && ts["exclusive"] == false && v["active"] != "off";
    if ts["enabled"] == true && !alongside {
        v["conflict"] = json!(v["active"] != "off");
        v["active"] = json!("tailscale");
    }
    v["alongside"] = json!(alongside);
    v["backends"].as_array_mut().unwrap().push(ts);
    v
}
fn status() -> Value {
    compose_tailscale(
        compose(wireguard::summary(), clash::summary()),
        crate::tailscale::summary(),
    )
}
pub fn get() -> (u16, Value) {
    (200, json!({"ok": true, "data": status()}))
}
/// The backend updates that `switch` asks for, in order; each backend is (saved as on,
/// current revision). With both on, the one not chosen (both for off) is turned off at
/// its current revision, since the client's revision cannot name two backends. Clash
/// goes first: it is not the one in effect, and turning WireGuard off first would let
/// the Clash supervisor start it before the second step.
fn requests(switch: &Switch, wireguard: (bool, u64), clash: (bool, u64)) -> Vec<(Backend, Value)> {
    let off = |b, rev| (b, json!({"expected_revision": rev, "enabled": false}));
    if wireguard.0 && clash.0 {
        let mut out = vec![];
        if switch.backend != Backend::Clash {
            out.push(off(Backend::Clash, clash.1));
        }
        if switch.backend != Backend::Wireguard {
            out.push(off(Backend::Wireguard, wireguard.1));
        }
        return out;
    }
    let rev = switch.expected_revision;
    match switch.backend {
        Backend::Off if wireguard.0 => vec![off(Backend::Wireguard, rev)],
        Backend::Off if clash.0 => vec![off(Backend::Clash, rev)],
        Backend::Off => vec![],
        b => vec![(
            b,
            json!({"expected_revision": rev, "enabled": true, "takeover": true}),
        )],
    }
}
/// Whether an off request names a backend other than the one that is on.
fn stale_off(switch: &Switch, wireguard_on: bool, clash_on: bool) -> bool {
    let on = match (wireguard_on, clash_on) {
        (true, false) => Backend::Wireguard,
        (false, true) => Backend::Clash,
        _ => return false,
    };
    switch.backend == Backend::Off && switch.active.is_some_and(|a| a != on)
}
pub fn set(body: &[u8]) -> (u16, Value) {
    let _transition = crate::transition::lock();
    let Ok(switch) = serde_json::from_slice::<Switch>(body) else {
        return (400, json!({"ok": false, "error": "Invalid proxy request"}));
    };
    let state = |s: Result<Value, String>| {
        s.map(|v| (v["enabled"] == true, v["revision"].as_u64().unwrap_or(0)))
            .unwrap_or((false, 0))
    };
    let (wireguard, clash) = (state(wireguard::summary()), state(clash::summary()));
    let ts = crate::tailscale::summary().ok();
    let ts_on = ts.as_ref().is_some_and(|s| s["enabled"] == true);
    // Shown as the active backend (compose_tailscale), so Off turns Tailscale off.
    let ts_active =
        ts_on && (ts.as_ref().is_some_and(|s| s["exclusive"] == true) || !wireguard.0 && !clash.0);
    // Off for a Tailscale that now runs beside Clash or WireGuard must not turn those off.
    if switch.backend == Backend::Off
        && !ts_active
        && (wireguard.0 || clash.0)
        && switch.active == Some(Backend::Tailscale)
    {
        return (
            409,
            json!({"ok":false,"error":"Configuration changed; refresh before saving"}),
        );
    }
    if switch.backend == Backend::Tailscale || (switch.backend == Backend::Off && ts_active) {
        if switch.backend == Backend::Off && switch.active.is_some_and(|a| a != Backend::Tailscale)
        {
            return (
                409,
                json!({"ok":false,"error":"Configuration changed; refresh before saving"}),
            );
        }
        let action = if switch.backend == Backend::Tailscale {
            "connect"
        } else {
            "disconnect"
        };
        let body = serde_json::to_vec(
            &json!({"action":action,"expected_revision":switch.expected_revision}),
        )
        .unwrap();
        let (code, reply) = crate::tailscale::update_locked(&body);
        if code != 200 {
            return (code, reply);
        }
        // Chosen here, Tailscale is the only backend on, also without an exit node (its
        // own Connect then leaves Clash and WireGuard on).
        if let Err(e) = crate::tailscale::native_off() {
            return (400, json!({"ok":false,"error":e}));
        }
        return get();
    }
    if ts_on && matches!(switch.backend, Backend::Wireguard | Backend::Clash) {
        let revision = if switch.backend == Backend::Wireguard {
            wireguard.1
        } else {
            clash.1
        };
        if revision != switch.expected_revision {
            return (
                409,
                json!({"ok":false,"error":"Configuration changed; refresh before saving"}),
            );
        }
        if let Err(e) = crate::tailscale::make_room() {
            return (400, json!({"ok":false,"error":e}));
        }
    }
    if stale_off(&switch, wireguard.0, clash.0) {
        return (
            409,
            json!({"ok": false, "error": "Configuration changed; refresh before saving"}),
        );
    }
    for (backend, update) in requests(&switch, wireguard, clash) {
        let body = serde_json::to_vec(&update).unwrap();
        let (code, reply) = match backend {
            Backend::Clash => clash::update_locked(&body),
            _ => wireguard::update_locked(&body),
        };
        if code != 200 {
            return (code, reply);
        }
    }
    get()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn summary(enabled: bool) -> Result<Value, String> {
        Ok(json!({"enabled": enabled, "revision": 3, "ready": true, "reason": null, "error": null}))
    }
    #[test]
    fn tailscale_mode_is_explicit_and_conflicts_are_visible() {
        let only_ts = compose_tailscale(compose(summary(false), summary(false)), summary(true));
        assert_eq!(only_ts["active"], "tailscale");
        assert_eq!(only_ts["conflict"], false);
        assert_eq!(only_ts["backends"].as_array().unwrap().len(), 3);
        let conflict = compose_tailscale(compose(summary(true), summary(false)), summary(true));
        assert_eq!(conflict["conflict"], true);
        assert_eq!(conflict["alongside"], false);
        assert!(
            serde_json::from_str::<Switch>(r#"{"backend":"tailscale","expected_revision":0}"#)
                .is_ok()
        );
        assert!(serde_json::from_str::<Switch>(
            r#"{"backend":"tailscale","expected_revision":0,"command":"up"}"#
        )
        .is_err());
    }
    #[test]
    fn tailscale_without_an_exit_node_runs_beside_the_native_backend() {
        let ts = |exclusive: bool| {
            summary(true).map(|mut v| {
                v["exclusive"] = json!(exclusive);
                v
            })
        };
        let beside = compose_tailscale(compose(summary(false), summary(true)), ts(false));
        assert_eq!(
            (&beside["active"], &beside["conflict"], &beside["alongside"]),
            (&json!("clash"), &json!(false), &json!(true))
        );
        let alone = compose_tailscale(compose(summary(false), summary(false)), ts(false));
        assert_eq!(
            (&alone["active"], &alone["alongside"]),
            (&json!("tailscale"), &json!(false))
        );
        let exit = compose_tailscale(compose(summary(true), summary(false)), ts(true));
        assert_eq!(
            (&exit["active"], &exit["conflict"], &exit["alongside"]),
            (&json!("tailscale"), &json!(true), &json!(false))
        );
        let off = compose_tailscale(compose(summary(false), summary(true)), summary(false));
        assert_eq!(
            (&off["active"], &off["alongside"]),
            (&json!("clash"), &json!(false))
        );
    }
    #[test]
    fn the_active_backend_follows_the_saved_state() {
        assert_eq!(compose(summary(false), summary(false))["active"], "off");
        assert_eq!(compose(summary(false), summary(true))["active"], "clash");
        let both = compose(summary(true), summary(true));
        assert_eq!(
            (both["active"].as_str(), both["conflict"].as_bool()),
            (Some("wireguard"), Some(true))
        );
        let broken = compose(
            Err("Saved WireGuard configuration is invalid".into()),
            summary(false),
        );
        assert_eq!(broken["backends"][0]["ready"], false);
        assert_eq!(
            broken["backends"][0]["reason"],
            "Saved WireGuard configuration is invalid"
        );
        assert_eq!(broken["backends"][1]["id"], "clash");
    }
    #[test]
    fn switches_map_to_backend_updates_with_takeover() {
        let parse = |s: &str| serde_json::from_slice::<Switch>(s.as_bytes());
        let clash = parse(r#"{"backend":"clash","expected_revision":7}"#).unwrap();
        let updates = requests(&clash, (true, 4), (false, 6));
        assert_eq!(
            updates,
            vec![(
                Backend::Clash,
                json!({"expected_revision": 7, "enabled": true, "takeover": true})
            )]
        );
        let off = parse(r#"{"backend":"off","expected_revision":2}"#).unwrap();
        assert_eq!(
            requests(&off, (false, 1), (true, 2)),
            vec![(
                Backend::Clash,
                json!({"expected_revision": 2, "enabled": false})
            )]
        );
        assert_eq!(
            requests(&off, (true, 2), (false, 1))[0].0,
            Backend::Wireguard
        );
        assert!(requests(&off, (false, 1), (false, 1)).is_empty());
        assert!(parse(r#"{"backend":"sing-box","expected_revision":1}"#).is_err());
        assert!(parse(r#"{"backend":"off","expected_revision":1,"takeover":true}"#).is_err());
    }
    #[test]
    fn off_must_name_the_backend_that_is_on() {
        let parse = |s: &str| serde_json::from_slice::<Switch>(s.as_bytes()).unwrap();
        let off_wg = parse(r#"{"backend":"off","expected_revision":3,"active":"wireguard"}"#);
        assert!(!stale_off(&off_wg, true, false));
        assert!(stale_off(&off_wg, false, true), "Clash came on elsewhere");
        assert!(!stale_off(&off_wg, false, false), "nothing on: already off");
        assert!(!stale_off(&off_wg, true, true), "a conflict turns both off");
        let legacy = parse(r#"{"backend":"off","expected_revision":3}"#);
        assert!(!stale_off(&legacy, false, true));
        let on = parse(r#"{"backend":"clash","expected_revision":3,"active":"wireguard"}"#);
        assert!(!stale_off(&on, false, true));
    }
    #[test]
    fn a_conflict_turns_off_the_backend_not_chosen() {
        let parse = |s: &str| serde_json::from_slice::<Switch>(s.as_bytes()).unwrap();
        let both = ((true, 4), (true, 9));
        let keep_wg = requests(
            &parse(r#"{"backend":"wireguard","expected_revision":4}"#),
            both.0,
            both.1,
        );
        assert_eq!(
            keep_wg,
            vec![(
                Backend::Clash,
                json!({"expected_revision": 9, "enabled": false})
            )]
        );
        let keep_clash = requests(
            &parse(r#"{"backend":"clash","expected_revision":9}"#),
            both.0,
            both.1,
        );
        assert_eq!(
            keep_clash,
            vec![(
                Backend::Wireguard,
                json!({"expected_revision": 4, "enabled": false})
            )]
        );
        let off = requests(
            &parse(r#"{"backend":"off","expected_revision":0}"#),
            both.0,
            both.1,
        );
        assert_eq!(
            off.iter().map(|u| u.0).collect::<Vec<_>>(),
            vec![Backend::Clash, Backend::Wireguard]
        );
    }
}
