<script>
  import { onMount } from 'svelte';
  import { api } from './api.js';
  import { toast } from './toast.svelte.js';
  import { copy, short, when, safeImage } from './util.js';
  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';

  let { identity, onsaved } = $props();

  let form = $state({ displayName: '', displayPicture: '', description: '' });
  /** @type {{ displayName?: string, displayPicture?: string, description?: string, updated?: number }} */
  let saved = $state({});
  let saving = $state(false);
  let broadcasting = $state(false);
  let loaded = $state(false);

  const dirty = $derived(
    ['displayName', 'displayPicture', 'description'].some((k) => (form[k] || '') !== (saved[k] || '')),
  );
  const pictureOk = $derived(!form.displayPicture.trim() || !!safeImage(form.displayPicture) && /^https?:/i.test(form.displayPicture.trim()));

  onMount(async () => {
    try {
      saved = await api.profile();
      form = { displayName: saved.displayName ?? '', displayPicture: saved.displayPicture ?? '', description: saved.description ?? '' };
    } catch (e) {
      toast(e.message, 'error');
    } finally {
      loaded = true;
    }
  });

  async function save(e) {
    e.preventDefault();
    saving = true;
    try {
      saved = await api.saveProfile(form);
      toast('Profile saved');
      onsaved?.();
    } catch (err) {
      toast(err.message, 'error');
    } finally {
      saving = false;
    }
  }

  // Send the (saved) profile to every completed connection and every DID with a chat.
  async function broadcast() {
    broadcasting = true;
    try {
      const [connections, conversations] = await Promise.all([api.connections(), api.conversations()]);
      const targets = new Set([
        ...connections.filter((c) => c.state === 'completed').map((c) => c.id),
        ...conversations.map((c) => c.peer),
      ]);
      let ok = 0, failed = 0;
      for (const t of targets) {
        try {
          await api.shareProfile(t, false);
          ok++;
        } catch {
          failed++;
        }
      }
      toast(targets.size ? `Profile sent to ${ok} peer${ok === 1 ? '' : 's'}${failed ? `, ${failed} failed` : ''}` : 'No peers to send it to yet', failed ? 'error' : 'info');
    } catch (err) {
      toast(err.message, 'error');
    } finally {
      broadcasting = false;
    }
  }
</script>

<div class="page scroll-thin">
  <h1>Profile</h1>
  <p class="sub muted">How peers see this agent. Shared with the <a href="https://didcomm.org/user-profile/1.0/" target="_blank" rel="noreferrer">User Profile 1.0</a> protocol: sent on request, and when you share it.</p>

  <div class="layout">
    <form class="card pad" onsubmit={save}>
      <label class="field">
        Display name
        <input class="input" bind:value={form.displayName} placeholder="e.g. Main agent" maxlength="80" />
      </label>
      <label class="field">
        Picture URL
        <span class="hint">An https:// link to an image. Peers fetch it themselves.</span>
        <input class="input" bind:value={form.displayPicture} placeholder="https://…/avatar.png" />
        {#if !pictureOk}<span class="error">Must be an http(s) URL.</span>{/if}
      </label>
      <label class="field">
        Description
        <textarea class="input" rows="4" bind:value={form.description} placeholder="A short bio" maxlength="500"></textarea>
      </label>
      <div class="row">
        <button class="btn primary" disabled={!loaded || !dirty || saving || !pictureOk}>{saving ? 'Saving…' : 'Save'}</button>
        <button type="button" class="btn" onclick={broadcast} disabled={broadcasting || dirty} title={dirty ? 'Save first' : 'Send to all your peers'}>
          <Icon name="share" size={16} /> {broadcasting ? 'Sending…' : 'Send to all peers'}
        </button>
        {#if saved.updated}<span class="muted small">Saved {when(saved.updated)}</span>{/if}
      </div>
    </form>

    <div class="preview">
      <div class="section-title">Preview</div>
      <div class="card me">
        <Avatar name={form.displayName || 'Agent'} picture={form.displayPicture} seed={identity?.did} size={88} />
        <div class="pname">{form.displayName || 'Unnamed agent'}</div>
        {#if form.description}<p class="muted">{form.description}</p>{/if}
      </div>

      <div class="section-title">Identity</div>
      <div class="card ident">
        <div class="kv">
          <span class="muted">DID</span>
          <span class="mono val" title={identity?.did}>{short(identity?.did, 12)}</span>
          <button class="icon-btn" title="Copy" onclick={() => copy(identity?.did).then(() => toast('DID copied'))}><Icon name="copy" size={16} /></button>
        </div>
        <div class="kv">
          <span class="muted">Method</span>
          <span class="val"><span class="pill accent">did:{identity?.did_method ?? '…'}</span></span>
        </div>
        {#if identity?.endpoint}
          <div class="kv"><span class="muted">Endpoint</span><span class="mono val">{identity.endpoint}</span></div>
        {/if}
        <div class="kv">
          <span class="muted">DIDComm v1</span>
          <span class="mono val" title={identity?.didcomm_v1?.did}>{short(identity?.didcomm_v1?.did, 10)}</span>
          <button class="icon-btn" title="Copy" onclick={() => copy(identity?.didcomm_v1?.did).then(() => toast('Copied'))}><Icon name="copy" size={16} /></button>
        </div>
      </div>
    </div>
  </div>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: minmax(0, 1.3fr) minmax(0, 1fr);
    gap: 1.5rem;
    align-items: start;
  }
  @media (max-width: 900px) {
    .layout {
      grid-template-columns: 1fr;
    }
  }
  .pad {
    padding: 1.5rem;
    display: grid;
    gap: 1.1rem;
  }
  .row {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .small {
    font-size: 0.82rem;
  }
  .error {
    color: var(--bad);
    font-size: 0.82rem;
    font-weight: 400;
  }
  .preview .section-title:first-child {
    margin-top: 0;
  }
  .me {
    padding: 1.75rem 1.25rem;
    display: grid;
    justify-items: center;
    text-align: center;
    gap: 0.6rem;
  }
  .pname {
    font-size: 1.2rem;
    font-weight: 650;
  }
  .me p {
    margin: 0;
    white-space: pre-wrap;
  }
  .ident {
    padding: 0.5rem 1rem;
  }
  .kv {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.55rem 0;
    border-bottom: 1px solid var(--border);
    font-size: 0.88rem;
  }
  .kv:last-child {
    border-bottom: none;
  }
  .kv > .muted {
    width: 90px;
    flex: none;
  }
  .val {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
