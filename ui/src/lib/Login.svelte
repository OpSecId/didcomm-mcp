<script>
  import { api } from './api.js';
  import Icon from './Icon.svelte';
  let { onsignedin } = $props();
  let token = $state('');
  let error = $state('');
  let busy = $state(false);

  async function submit(e) {
    e.preventDefault();
    busy = true;
    error = '';
    try {
      await api.login(token);
      token = '';
      onsignedin();
    } catch (err) {
      error = err.status === 401 ? 'That token isn’t right.' : err.message;
    } finally {
      busy = false;
    }
  }
</script>

<div class="wrap">
  <form class="card" onsubmit={submit}>
    <div class="logo"><Icon name="shield" size={26} /></div>
    <h1>Sign in</h1>
    <p class="muted">Enter this server’s access token (<code>DIDCOMM_MCP_HTTP_TOKEN</code>).</p>
    <label class="field">
      Access token
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input mono" type="password" autocomplete="current-password" bind:value={token} autofocus required />
    </label>
    {#if error}<p class="error">{error}</p>{/if}
    <button class="btn primary" disabled={busy || !token}>{busy ? 'Signing in…' : 'Sign in'}</button>
  </form>
</div>

<style>
  .wrap {
    min-height: 100%;
    display: grid;
    place-items: center;
    padding: 1.5rem;
    background: radial-gradient(1200px 600px at 20% -10%, var(--accent-soft), transparent), var(--bg);
  }
  form {
    width: min(400px, 100%);
    padding: 2rem;
    display: grid;
    gap: 1rem;
  }
  .logo {
    width: 48px;
    height: 48px;
    border-radius: 14px;
    display: grid;
    place-items: center;
    background: var(--accent-2);
    color: white;
  }
  h1 {
    margin: 0;
    font-size: 1.4rem;
  }
  p {
    margin: 0;
  }
  .error {
    color: var(--bad);
    font-size: 0.9rem;
  }
  button {
    justify-content: center;
    padding: 0.65rem;
  }
</style>
