import type { TailscaleStatus, TailscaleUpdate } from '../../types'

/** Only current eligible online peers can be offered; the agent validates again. */
export function eligibleExitNodes(status: TailscaleStatus) {
  const seen = new Set<string>()
  return status.exit_nodes.filter(node => {
    if (node.online !== true || !node.id || seen.has(node.id)) return false
    seen.add(node.id)
    return true
  })
}

export interface SignIn { hostname: string; authKey: string }

/** What tailscale accepts for --hostname; empty keeps the router's own name. */
export const validHostname = (s: string) => s === '' || /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(s)
/** Empty signs in through a browser link instead. */
export const validAuthKey = (s: string) => s === '' || /^tskey-[A-Za-z0-9_-]{10,250}$/.test(s)

export function tailscaleUpdate(status: TailscaleStatus, action: TailscaleUpdate['action'], exitNode = '', signIn?: SignIn): TailscaleUpdate {
  if (action === 'login') {
    const hostname = signIn?.hostname.trim() ?? '', authKey = signIn?.authKey.trim() ?? ''
    if (!validHostname(hostname)) throw new Error('Device name may use only letters, digits and hyphens (up to 63)') // i18n-ignore: translated at display
    if (!validAuthKey(authKey)) throw new Error('Auth key must be a tskey-… key from the Tailscale admin console') // i18n-ignore: translated at display
    return { expected_revision: status.revision, action, ...(hostname ? { hostname } : {}), ...(authKey ? { auth_key: authKey } : {}) }
  }
  if (action === 'select_exit') {
    if (exitNode !== '' && !eligibleExitNodes(status).some(node => node.id === exitNode)) {
      throw new Error('Choose a current online exit node') // i18n-ignore: translated at display
    }
    return { expected_revision: status.revision, action, exit_node: exitNode }
  }
  if (action === 'remote_access') {
    return { expected_revision: status.revision, action, enabled: !status.remote_access }
  }
  return { expected_revision: status.revision, action }
}

/** Where remote management stands: off, waiting for admin approval, working, or failed. */
export function remoteAccessState(status: TailscaleStatus): 'unsupported' | 'off' | 'error' | 'starting' | 'pending' | 'active' {
  if (status.remote_access === undefined) return 'unsupported'
  if (!status.remote_access) return 'off'
  const lan = status.lan_cidr ?? ''
  if (status.remote_error || !lan) return 'error'
  if (status.backend_state !== 'Running' || !(status.advertised_routes ?? []).includes(lan)) return 'starting'
  return status.route_approved ? 'active' : 'pending'
}

/** The first-run stage: daemon missing, signing in, awaiting device approval, or signed in. */
export function setupStage(status: TailscaleStatus): 'unsupported' | 'unavailable' | 'signin' | 'waiting' | 'approval' | 'ready' {
  if (status.login_active === undefined) return 'unsupported'
  if (status.installed === false || status.backend_state === 'Unavailable') return 'unavailable'
  if (status.self && (status.backend_state === 'Running' || status.backend_state === 'Stopped')) return 'ready'
  if (status.backend_state === 'NeedsMachineAuth') return 'approval'
  return status.login_active ? 'waiting' : 'signin'
}
