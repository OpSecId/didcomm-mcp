// A tiny hash-free router over history.pushState: /chats, /chats/<peer>, /connections,
// /profile, /system.

function parse() {
  const parts = location.pathname.split('/').filter(Boolean).map(decodeURIComponent);
  return { page: parts[0] || 'chats', id: parts[1] ?? null };
}

export const route = $state(parse());

export function go(page, id = null) {
  const path = '/' + [page, id].filter((p) => p != null).map(encodeURIComponent).join('/');
  if (path !== location.pathname) history.pushState({}, '', path);
  Object.assign(route, { page, id });
}

addEventListener('popstate', () => Object.assign(route, parse()));
