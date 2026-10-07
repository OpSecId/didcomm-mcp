<script>
  // A QR code as an SVG drawn from uqr's module matrix: no innerHTML, and always dark
  // modules on white (with a quiet zone) so phone cameras read it in dark mode too.
  import { encode } from 'uqr';

  // `size` is a minimum: dense codes (long invitation URLs) grow so every module gets
  // at least `minModule` pixels, which cameras need to read them off a screen.
  let { value, size = 220, minModule = 4, label = 'QR code' } = $props();

  const qr = $derived.by(() => {
    try {
      return encode(value, { ecc: 'L', border: 4 });
    } catch {
      return null; // too long for a QR code
    }
  });

  const pixels = $derived(qr ? Math.max(size, qr.size * minModule) : size);

  // One path of 1×1 squares, merged per row into horizontal runs.
  const path = $derived.by(() => {
    if (!qr) return '';
    let d = '';
    qr.data.forEach((row, y) => {
      let x = 0;
      while (x < row.length) {
        if (!row[x]) {
          x++;
          continue;
        }
        const start = x;
        while (x < row.length && row[x]) x++;
        d += `M${start} ${y}h${x - start}v1h-${x - start}z`;
      }
    });
    return d;
  });
</script>

{#if qr}
  <svg class="qr" width={pixels} height={pixels} viewBox="0 0 {qr.size} {qr.size}" role="img" aria-label={label} shape-rendering="crispEdges">
    <rect width={qr.size} height={qr.size} fill="#fff" />
    <path d={path} fill="#000" />
  </svg>
{:else}
  <div class="too-long muted" style="width:{size}px;height:{size}px">Too long for a QR code — use the link.</div>
{/if}

<style>
  .qr {
    display: block;
    border-radius: 12px;
    max-width: 100%;
    height: auto;
  }
  .too-long {
    display: grid;
    place-items: center;
    text-align: center;
    font-size: 0.85rem;
    border: 1px dashed var(--border);
    border-radius: 12px;
    padding: 1rem;
  }
</style>
