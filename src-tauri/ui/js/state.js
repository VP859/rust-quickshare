// State shared by the parts of the interface. Each module replaces the fields
// it owns (e.g. peers.js sets `state.peers`) and the others only read them.
export const state = {
  /** `get_node_info`: Node ID, TICKET and QR code, download folder; null until loaded. */
  nodeInfo: null,
  /** 'starting', 'ok' or 'bad'. */
  nodeState: 'starting',
  /** Trusted devices (`list_peers`). */
  peers: [],
  /** Node ID → reachable (true/false); missing while it is being checked. */
  online: new Map(),
  /** Devices found on the local network (`nearby` events). */
  nearby: [],
  /** LocalSend apps heard on the local network (`localsend-nearby` events). */
  localsend: [],
  /** The network mode the node runs in and the saved one. */
  network: { active: 'local', saved: 'local' },
  /** Privacy settings (`get_privacy`). */
  privacy: { hidden: false, auto_accept_texts: false, auto_accept_executables: false, localsend: false, keep_history: true },
};

export const isTrusted = (id) => state.peers.some(p => p.node_id === id);

const PLATFORMS = { linux: 'Linux', windows: 'Windows', macos: 'macOS', android: 'Android' };
export const platformLabel = (p) => PLATFORMS[p] ?? p;
export const platformIcon = (p) => p === 'android' ? 'phone' : 'monitor';
