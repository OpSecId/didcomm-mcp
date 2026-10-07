export const toasts = $state([]);
let next = 1;

export function toast(text, kind = 'info') {
  const id = next++;
  toasts.push({ id, text, kind });
  setTimeout(() => {
    const i = toasts.findIndex((t) => t.id === id);
    if (i >= 0) toasts.splice(i, 1);
  }, kind === 'error' ? 6000 : 3000);
}
