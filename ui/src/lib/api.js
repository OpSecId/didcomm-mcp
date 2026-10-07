// The server's /api, with the session cookie. A 401 sends the app back to sign-in.

/** @type {() => void} */
let onUnauthorized = () => {};
export function setUnauthorizedHandler(fn) {
  onUnauthorized = fn;
}

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

async function request(method, path, body) {
  const res = await fetch(path, {
    method,
    credentials: 'same-origin',
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  let data = null;
  try {
    data = await res.json();
  } catch {
    /* empty body */
  }
  if (res.status === 401 && path !== '/api/login') onUnauthorized();
  if (!res.ok) throw new ApiError(res.status, data?.error ?? `${res.status} ${res.statusText}`);
  return data;
}

export const api = {
  session: () => request('GET', '/api/session'),
  login: (token) => request('POST', '/api/login', { token }),
  logout: () => request('POST', '/api/logout', {}),
  identity: () => request('GET', '/api/identity'),
  profile: () => request('GET', '/api/profile'),
  saveProfile: (p) => request('PUT', '/api/profile', p),
  shareProfile: (target, send_back_yours = true) => request('POST', '/api/profile/share', { target, send_back_yours }),
  requestProfile: (target) => request('POST', '/api/profile/request', { target }),
  connections: () => request('GET', '/api/connections'),
  acceptInvitation: (invitation) => request('POST', '/api/connections/accept', { invitation }),
  createInvitation: (didcomm_version = 'v1', validity_seconds) => request('POST', '/api/invitations', { didcomm_version, validity_seconds }),
  invitations: () => request('GET', '/api/invitations'),
  revokeInvitation: (id) => request('POST', '/api/invitations/revoke', { id }),
  conversations: () => request('GET', '/api/conversations'),
  messages: (peer, params = {}) => {
    const q = new URLSearchParams({ peer, ...Object.fromEntries(Object.entries(params).filter(([, v]) => v != null).map(([k, v]) => [k, String(v)])) });
    return request('GET', `/api/messages?${q}`);
  },
  send: (target, content) => request('POST', '/api/messages', { target, content }),
  markRead: (peer, upto) => request('POST', '/api/read', { peer, upto }),
  refresh: () => request('POST', '/api/refresh', {}),
  status: () => request('GET', '/api/status'),
  config: () => request('GET', '/api/config'),
};
