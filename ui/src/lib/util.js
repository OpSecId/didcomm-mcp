// Display helpers.

/** A short form of a long DID or id: did:peer:4zQm…Xyz9 */
export function short(value, keep = 6) {
  if (!value) return '';
  if (value.length <= keep * 2 + 12) return value;
  const colon = value.startsWith('did:') ? value.indexOf(':', 4) + 1 : 0;
  const head = value.slice(0, colon + keep);
  return `${head}…${value.slice(-keep)}`;
}

/** Initials for an avatar. */
export function initials(name) {
  const words = (name || '').trim().split(/\s+/).filter(Boolean);
  if (!words.length) return '?';
  return (words[0][0] + (words.length > 1 ? words[words.length - 1][0] : '')).toUpperCase();
}

/** A stable hue for a peer, so avatars keep their colour. */
export function hue(key) {
  let h = 0;
  for (const c of key || '') h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

/** The text of a history entry's message, whatever its protocol and version. */
export function messageText(entry) {
  const m = entry?.entry?.message ?? {};
  const content = m.body?.content ?? m.content;
  if (typeof content === 'string') return content;
  const type = m.type ?? m['@type'] ?? 'message';
  return `[${type.replace(/^https:\/\/didcomm\.org\//, '')}]`;
}

/** The protocol name of a history entry's message, for non-text messages. */
export function messageType(entry) {
  const m = entry?.entry?.message ?? {};
  return m.type ?? m['@type'] ?? '';
}

export function isText(entry) {
  const t = messageType(entry);
  return /basicmessage\/\d/.test(t);
}

/** "14:03", "Yesterday", "Mon", "12 Oct" */
export function when(seconds, withTime = false) {
  if (!seconds) return '';
  const d = new Date(seconds * 1000);
  const now = new Date();
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  if (withTime) return time;
  const days = Math.floor((startOfDay(now) - startOfDay(d)) / 86_400_000);
  if (days === 0) return time;
  if (days === 1) return 'Yesterday';
  if (days < 7) return d.toLocaleDateString([], { weekday: 'short' });
  return d.toLocaleDateString([], { day: 'numeric', month: 'short' });
}

/** A day heading for the message list: "Today", "Yesterday", "Monday 12 October". */
export function dayLabel(seconds) {
  const d = new Date(seconds * 1000);
  const days = Math.floor((startOfDay(new Date()) - startOfDay(d)) / 86_400_000);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  return d.toLocaleDateString([], { weekday: 'long', day: 'numeric', month: 'long', year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric' });
}

function startOfDay(d) {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

export function duration(seconds) {
  if (seconds == null) return '';
  const d = Math.floor(seconds / 86400), h = Math.floor((seconds % 86400) / 3600), m = Math.floor((seconds % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  return `${m}m ${seconds % 60}s`;
}

/**
 * The name to show for a peer: their profile's, the connection's label, or a short DID.
 * @param {string} peer
 * @param {{ profile?: any, connection?: any }} [known]
 */
export function peerName(peer, { profile, connection } = {}) {
  return profile?.displayName || connection?.their_label || short(connection?.their_did || peer);
}

export async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** Only http(s) and data:image URLs are shown as pictures. */
export function safeImage(url) {
  if (typeof url !== 'string') return null;
  const u = url.trim();
  if (/^https?:\/\//i.test(u) || /^data:image\/(png|jpe?g|gif|webp);base64,/i.test(u)) return u;
  return null;
}
