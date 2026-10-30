//! Pure fixture-only adapter parsers: never execute a process or access fixed
//! production paths, identifiers, libraries, EFS or NV.
use super::*;
fn record() -> Vec<u8> {
    let base = "00000000000001";
    let value = (0..10)
        .map(|n| format!("{base}{n}"))
        .find(|v| valid_imei(v))
        .unwrap();
    let ds: Vec<_> = value.bytes().map(|b| b - b'0').collect();
    let mut r = vec![0x5a; 128];
    r[0] = 8;
    r[1] = ds[0] << 4 | 10;
    for i in 0..7 {
        r[i + 2] = ds[1 + 2 * i] | ds[2 + 2 * i] << 4;
    }
    r
}
#[test]
fn duplicate_or_missing_authority_fails() {
    let good = "PAIR_RESULT outcome=0 released=1 deinit=1 signal=0\n";
    assert!(require_fields(
        &format!("{good}{good}"),
        "PAIR_RESULT ",
        &[("outcome", "0")]
    )
    .is_err());
    assert!(require_fields(
        "PAIR_RESULT outcome=0 released=0 deinit=1 signal=0\n",
        "PAIR_RESULT ",
        &[("released", "1")]
    )
    .is_err());
    assert!(require_fields(
        "PAIR_RESULT outcome=0 outcome=0 released=1\n",
        "PAIR_RESULT ",
        &[("outcome", "0")]
    )
    .is_err());
    assert!(require_fields(
        "PAIR_RESULT outcome=0\n",
        "PAIR_RESULT ",
        &[("deinit", "1")]
    )
    .is_err());
    assert!(parse_nv("APP_NV_TOPOLOGY count=1 secondary=not-active-status5\n").is_err());
}
#[test]
fn nv_records_require_exact_topology_and_cleanup() {
    let r = record();
    let data = r.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let topology = "APP_NV_TOPOLOGY count=1 secondary=not-active-status5\n";
    let line = format!("APP_NV index=0 data={data}\n");
    let authority = "PAIR_RESULT outcome=0 released=1 deinit=1 signal=0\n";
    assert_eq!(
        parse_nv(&format!("{topology}{line}{authority}")).unwrap(),
        vec![r]
    );
    assert!(parse_nv(&format!("{topology}{line}{line}{authority}")).is_err());
    assert!(parse_nv(&format!(
        "{topology}{line}PAIR_RESULT outcome=0 released=1 deinit=0 signal=0\n"
    ))
    .is_err());
    assert!(parse_nv(&format!(
        "APP_NV_TOPOLOGY count=2 secondary=active\n{line}{authority}"
    ))
    .is_err());
    assert!(parse_nv(&format!(
        "APP_NV_TOPOLOGY count=1 secondary=unknown\n{line}{authority}"
    ))
    .is_err());
}
fn metadata() -> Value {
    serde_json::json!({"schema":1,"profile":PROFILE,"source_commit":SOURCE,"router_sha256":ROUTER_HASH,
        "libdiag_sha256":LIBRARY_HASH,"musl_sha256":MUSL_HASH,"platform":"aarch64-linux-musl",
        "interpreter":"/lib/ld-musl-aarch64.so.1","helpers":{"zte_config":"1".repeat(64),"zte_config_read":"2".repeat(64),"zte_nv":"3".repeat(64)}})
}
#[test]
fn manifest_pins_platform_hashes_and_allowlisted_helper_set() {
    let m = metadata();
    assert!(manifest(&serde_json::to_vec(&m).unwrap()).is_ok());
    for key in [
        "profile",
        "source_commit",
        "router_sha256",
        "libdiag_sha256",
        "musl_sha256",
        "platform",
        "interpreter",
    ] {
        let mut bad = m.clone();
        bad[key] = serde_json::json!("wrong");
        assert!(manifest(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut bad = m.clone();
    bad["helpers"]["extra"] = serde_json::json!("4".repeat(64));
    assert!(manifest(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = m;
    bad["helpers"]["zte_nv"] = serde_json::json!("bad");
    assert!(manifest(&serde_json::to_vec(&bad).unwrap()).is_err());
}
fn binary() -> Vec<u8> {
    let mut b = vec![0u8; 256];
    b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    b[16] = 3;
    b[18] = 183;
    b[32..40].copy_from_slice(&64u64.to_le_bytes());
    b[54] = 56;
    b[56] = 1;
    b[64] = 3;
    b[72..80].copy_from_slice(&128u64.to_le_bytes());
    let path = b"/lib/ld-musl-aarch64.so.1\0";
    b[96..104].copy_from_slice(&(path.len() as u64).to_le_bytes());
    b[128..128 + path.len()].copy_from_slice(path);
    b
}
#[test]
fn executable_requires_real_aarch64_dynamic_interpreter() {
    let b = binary();
    assert!(elf(&b));
    let mut no_interp = b.clone();
    no_interp[64] = 0;
    assert!(!elf(&no_interp));
    let mut wrong_arch = b.clone();
    wrong_arch[18] = 62;
    assert!(!elf(&wrong_arch));
    let mut static_bin = b.clone();
    static_bin[16] = 2;
    assert!(!elf(&static_bin));
    let mut wrong_path = b.clone();
    wrong_path[130] = b'x';
    assert!(!elf(&wrong_path));
    let mut overflow = b;
    overflow[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(!elf(&overflow));
    assert!(!elf(&[]));
}
fn config_stream() -> String {
    fn put(d: &mut [u8], at: usize, v: u32) {
        d[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    let mut d = vec![0u8; CONFIG_SIZE];
    put(&mut d, 0, 0x78563412);
    put(&mut d, 4, 249);
    put(&mut d, 8, CONFIG_SIZE as u32);
    let mut off = 16;
    for i in 0..249 {
        let len = if i == 4 {
            403
        } else if i == 5 {
            17
        } else if i == 248 {
            10697
        } else {
            16
        };
        let id = if i == 5 { 102 } else { 1000 + i as u32 };
        put(&mut d, off, id);
        put(&mut d, off + 4, len as u32);
        put(&mut d, off + 8, 0x18080820);
        off += len;
    }
    put(&mut d, off, 0x21436587);
    let crc = config_crc(&d);
    put(&mut d, 12, crc);
    let mut text = String::from("EFS_STAT errno=0 size=15073 nlink=1 mode=0x8d00\n");
    for (i, c) in d.chunks(1024).enumerate() {
        let data = c.iter().map(|v| format!("{v:02x}")).collect::<String>();
        text.push_str(&format!(
            "EFS_DATA_HEX offset={} length={} data={}\n",
            i * 1024,
            c.len(),
            data
        ));
    }
    text.push_str("EFS_STAT errno=0 size=15073 nlink=1 mode=0x8d00\nEFS_FILE_COMPLETE path=/config length=15073\nEFS_RESULT outcome=0 bytes=15073 file_closed=1 released=1 deinit=1 signal=0 bad_crc_frames=0\n");
    text
}
#[test]
fn config_complete_stream_requires_stable_stat_and_cleanup() {
    let good = config_stream();
    let (d, m) = parse_config(&good).unwrap();
    assert_eq!(d.len(), CONFIG_SIZE);
    assert_eq!(m, 0x8d00);
    assert!(parse_config(&good.replace("deinit=1", "deinit=0")).is_err());
    assert!(parse_config(&good.replacen("mode=0x8d00", "mode=0x8d20", 1)).is_err());
    assert!(parse_config(&good.replace("nlink=1", "nlink=2")).is_err());
    assert!(parse_config(&good.replace("bad_crc_frames=0", "bad_crc_frames=1")).is_err());
}
#[test]
fn config_stream_rejects_gaps_repeats_and_incomplete_cleanup() {
    assert!(parse_config("EFS_DATA_HEX offset=1 length=1 data=00\n").is_err());
    assert!(parse_config(
        "EFS_DATA_HEX offset=0 length=1 data=00\nEFS_DATA_HEX offset=0 length=1 data=00\n"
    )
    .is_err());
    assert!(parse_config("EFS_DATA_HEX offset=0 length=1025 data=00\n").is_err());
    assert!(parse_config("EFS_RESULT outcome=0 bytes=15073 file_closed=1 released=1 deinit=0 signal=0 bad_crc_frames=0\n").is_err());
}
