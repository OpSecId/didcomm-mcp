<script>
  import { initials, hue, safeImage } from './util.js';
  let { name = '', picture = null, seed = '', size = 40 } = $props();
  let failed = $state(false);
  const src = $derived(failed ? null : safeImage(picture));
</script>

<span class="avatar" style="width:{size}px;height:{size}px;font-size:{size * 0.38}px;--h:{hue(seed || name)}">
  {#if src}
    <img {src} alt="" referrerpolicy="no-referrer" onerror={() => (failed = true)} />
  {:else}
    {initials(name)}
  {/if}
</span>

<style>
  .avatar {
    flex: none;
    display: inline-grid;
    place-items: center;
    border-radius: 50%;
    overflow: hidden;
    font-weight: 650;
    color: hsl(var(--h) 55% 32%);
    background: hsl(var(--h) 70% 88%);
    user-select: none;
  }
  @media (prefers-color-scheme: dark) {
    .avatar {
      color: hsl(var(--h) 70% 85%);
      background: hsl(var(--h) 35% 28%);
    }
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
</style>
