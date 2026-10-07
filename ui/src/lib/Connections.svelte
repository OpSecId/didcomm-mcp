<script>
  import { onMount } from 'svelte';
  import { api } from './api.js';
  import { go } from './router.svelte.js';
  import { toast } from './toast.svelte.js';
  import { peerName, short, copy, when } from './util.js';
  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';
  import QrCode from './QrCode.svelte';

  let connections = $state([]);
  let loading = $state(true);
  let invitation = $state(null);
  let creating = $state(false);
  let accepting = $state(false);
  let incoming = $state('');
  let copied = $state(false);
  let enlarged = $state(false);
  let version = $state('v2');
  let validity = $state(7 * 24 * 3600);
  let invitations = $state([]);

  // What to hand over: the short URL when the server has one (public URL set).
  const shareUrl = $derived(invitation ? invitation.short_url ?? invitation.invitation_url : '');
  const live = $derived(invitations.filter((i) => i.live));

  const validities = [
    [3600, '1 hour'],
    [24 * 3600, '1 day'],
    [7 * 24 * 3600, '7 days'],
    [30 * 24 * 3600, '30 days'],
    [0, 'Until revoked'],
  ];

  async function loadInvitations() {
    try {
      invitations = await api.invitations();
    } catch {
      /* listed again later */
    }
  }

  async function revoke(id) {
    try {
      await api.revokeInvitation(id);
      if (invitation?.id === id) invitation = null;
      toast('Invitation revoked');
      await loadInvitations();
    } catch (e) {
      toast(e.message, 'error');
    }
  }

  function expiry(seconds) {
    if (seconds == null) return 'never expires';
    const left = seconds - Date.now() / 1000;
    if (left <= 0) return 'expired';
    if (left < 3600) return `expires in ${Math.ceil(left / 60)} min`;
    if (left < 48 * 3600) return `expires in ${Math.round(left / 3600)} h`;
    return `expires in ${Math.round(left / 86400)} days`;
  }

  /** The invitation URL shortened in the middle: https://agent.didcomm.link/didcomm?oob=eyJ…Q30 */
  function truncateMiddle(url, head = 42, tail = 10) {
    return url.length <= head + tail + 1 ? url : `${url.slice(0, head)}…${url.slice(-tail)}`;
  }

  async function copyInvitation() {
    copied = await copy(shareUrl);
    toast(copied ? 'Invitation copied' : 'Copy failed', copied ? 'info' : 'error');
    if (copied) setTimeout(() => (copied = false), 2000);
  }

  async function load() {
    try {
      connections = await api.connections();
    } catch (e) {
      toast(e.message, 'error');
    } finally {
      loading = false;
    }
  }

  async function createInvitation() {
    creating = true;
    try {
      invitation = await api.createInvitation(version, validity);
      copied = false;
      await loadInvitations();
    } catch (e) {
      toast(e.message, 'error');
    } finally {
      creating = false;
    }
  }

  async function accept(e) {
    e.preventDefault();
    accepting = true;
    try {
      const c = await api.acceptInvitation(incoming.trim());
      incoming = '';
      toast(`Connected${c.their_label ? ' to ' + c.their_label : ''}`);
      await load();
    } catch (err) {
      toast(err.message, 'error');
    } finally {
      accepting = false;
    }
  }

  async function askProfile(c) {
    try {
      await api.requestProfile(c.id);
      toast('Profile requested');
      setTimeout(load, 1500);
    } catch (e) {
      toast(e.message, 'error');
    }
  }

  onMount(() => {
    load();
    loadInvitations();
  });
</script>

<div class="page scroll-thin">
  <h1>Connections</h1>
  <p class="sub muted">Agents you’re connected to over DIDComm v1 (DID Exchange). DIDComm v2 peers need no connection: open a chat with their DID.</p>

  <div class="grid2">
    <div class="card pad">
      <h3><Icon name="link" size={18} /> Invite someone</h3>
      <p class="muted">An out-of-band invitation for another agent to connect to this one.</p>
      <div class="options">
        <div class="seg" role="radiogroup" aria-label="DIDComm version">
          <button type="button" role="radio" aria-checked={version === 'v2'} class:on={version === 'v2'} onclick={() => (version = 'v2')} title="Out-of-Band 2.0: the peer messages this agent's DID">DIDComm v2</button>
          <button type="button" role="radio" aria-checked={version === 'v1'} class:on={version === 'v1'} onclick={() => (version = 'v1')} title="Out-of-Band 1.1 with DID Exchange (Aries agents)">DIDComm v1</button>
        </div>
        <select class="input expiry" bind:value={validity} aria-label="Short link validity">
          {#each validities as [seconds, text]}<option value={seconds}>{text}</option>{/each}
        </select>
      </div>
      {#if invitation}
        <div class="invitation">
          <button class="qr-btn" onclick={() => (enlarged = true)} title="Show larger for scanning">
            <QrCode value={shareUrl} size={240} label="Invitation QR code" />
          </button>
          <p class="muted scan">
            <span class="pill accent">DIDComm {invitation.didcomm_version}</span>
            {#if invitation.short_url}<span class="pill">{expiry(invitation.expires_time)}</span>{/if}
          </p>
          <div class="invite">
            <code title={shareUrl}>{truncateMiddle(shareUrl)}</code>
            <button class="icon-btn" class:done={copied} onclick={copyInvitation} title="Copy invitation URL" aria-label="Copy invitation URL">
              <Icon name={copied ? 'check' : 'copy'} size={18} />
            </button>
          </div>
        </div>
      {/if}
      <button class="btn primary" onclick={createInvitation} disabled={creating}>{creating ? 'Creating…' : invitation ? 'New invitation' : 'Create invitation'}</button>
    </div>
    <form class="card pad" onsubmit={accept}>
      <h3><Icon name="plus" size={18} /> Accept an invitation</h3>
      <p class="muted">Paste an invitation URL or its JSON.</p>
      <textarea class="input mono" rows="3" bind:value={incoming} placeholder="https://…?oob=…, ?_oob=…, ?_oobid=…"></textarea>
      <button class="btn primary" disabled={!incoming.trim() || accepting}>{accepting ? 'Connecting…' : 'Connect'}</button>
    </form>
  </div>

  {#if enlarged && invitation}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="overlay" onclick={() => (enlarged = false)}>
      <div class="big" role="dialog" aria-label="Invitation QR code">
        <QrCode value={shareUrl} size={Math.min(560, innerWidth - 48, innerHeight - 140)} label="Invitation QR code" />
        <button class="btn" onclick={() => (enlarged = false)}><Icon name="x" size={16} /> Close</button>
      </div>
    </div>
  {/if}

  {#if live.length}
    <div class="section-title">{live.length} active invitation{live.length === 1 ? '' : 's'}</div>
    <div class="card list">
      {#each live as inv (inv.id)}
        <div class="conn inv">
          <span class="pill accent">{inv.didcomm_version}</span>
          <div class="info">
            <code class="mono" title={inv.short_url}>{truncateMiddle(inv.short_url, 46, 8)}</code>
            <span class="muted small">{inv.label ?? 'didcomm-mcp'} · {when(inv.created_at)} · {expiry(inv.expires_time)}</span>
          </div>
          <div class="btns">
            <button class="icon-btn" title="Copy" aria-label="Copy" onclick={() => copy(inv.short_url).then((ok) => toast(ok ? 'Copied' : 'Copy failed'))}><Icon name="copy" size={16} /></button>
            <button class="btn small ghost danger" onclick={() => revoke(inv.id)}><Icon name="x" size={16} /> Revoke</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}

  <div class="section-title">{connections.length} connection{connections.length === 1 ? '' : 's'}</div>
  {#if loading}
    <div class="empty">Loading…</div>
  {:else if !connections.length}
    <div class="card empty">No connections yet.</div>
  {:else}
    <div class="card list">
      {#each connections as c (c.id)}
        {@const name = peerName(c.id, { profile: c.profile, connection: c })}
        <div class="conn">
          <Avatar {name} picture={c.profile?.displayPicture} seed={c.id} size={44} />
          <div class="info">
            <div class="name">{name}</div>
            {#if c.profile?.description}<div class="muted desc">{c.profile.description}</div>{/if}
            <div class="tags">
              <span class="pill {c.state === 'completed' ? 'ok' : ''}">{c.state}</span>
              <span class="pill">DIDComm {c.didcomm_version}</span>
              <span class="pill">{c.role}</span>
              {#if c.profile?.updated}<span class="pill accent">profile · {when(c.profile.updated)}</span>{/if}
            </div>
            <div class="did mono muted" title={c.their_did}>{short(c.their_did, 10)}</div>
          </div>
          <div class="btns">
            <button class="btn small" onclick={() => go('chats', c.id)}><Icon name="chat" size={16} /> Chat</button>
            <button class="btn small ghost" onclick={() => askProfile(c)} title="Ask for their profile (user-profile/1.0)"><Icon name="user" size={16} /> Profile</button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .grid2 {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
    gap: 1rem;
  }
  .pad {
    padding: 1.25rem;
    display: grid;
    gap: 0.75rem;
    align-content: start;
  }
  h3 {
    margin: 0;
    font-size: 1.02rem;
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .pad p {
    margin: 0;
    font-size: 0.9rem;
  }
  .pad .btn {
    justify-self: start;
  }
  .invitation {
    display: grid;
    justify-items: center;
    gap: 0.75rem;
    padding: 1rem;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--panel-2);
  }
  .qr-btn {
    border: none;
    padding: 0;
    background: none;
    cursor: zoom-in;
    border-radius: 12px;
  }
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    background: rgba(10, 12, 20, 0.72);
    cursor: zoom-out;
  }
  .big {
    display: grid;
    justify-items: center;
    gap: 1rem;
    padding: 1.25rem;
    background: #fff;
    border-radius: 18px;
  }
  .big .btn {
    background: #fff;
    color: #171a26;
    border-color: #e3e6ef;
  }
  .options {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    align-items: center;
  }
  .seg {
    display: inline-flex;
    padding: 3px;
    border-radius: 10px;
    background: var(--panel-2);
    border: 1px solid var(--border);
  }
  .seg button {
    border: none;
    background: none;
    padding: 0.35rem 0.75rem;
    border-radius: 7px;
    cursor: pointer;
    font-weight: 600;
    font-size: 0.85rem;
    color: var(--muted);
  }
  .seg button.on {
    background: var(--panel);
    color: var(--text);
    box-shadow: var(--shadow);
  }
  .expiry {
    width: auto;
    padding: 0.4rem 0.6rem;
    font-size: 0.88rem;
  }
  .inv {
    padding: 0.75rem 1.25rem;
  }
  .inv code {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .small {
    font-size: 0.8rem;
  }
  .danger:hover {
    color: var(--bad);
  }
  .scan {
    display: flex;
    gap: 0.4rem;
    font-size: 0.85rem;
    text-align: center;
  }
  .invite {
    width: 100%;
    display: flex;
    gap: 0.25rem;
    align-items: center;
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 0.3rem 0.3rem 0.3rem 0.7rem;
  }
  .invite code {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .icon-btn.done {
    color: var(--ok);
  }
  .list {
    overflow: hidden;
  }
  .conn {
    display: flex;
    gap: 1rem;
    align-items: center;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border);
  }
  .conn:last-child {
    border-bottom: none;
  }
  .info {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 0.3rem;
  }
  .name {
    font-weight: 600;
  }
  .desc {
    font-size: 0.88rem;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .did {
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .btns {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
    justify-content: flex-end;
  }
</style>
