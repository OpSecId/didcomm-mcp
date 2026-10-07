<script>
  import { onMount, tick } from 'svelte';
  import { api } from './api.js';
  import { route, go } from './router.svelte.js';
  import { toast } from './toast.svelte.js';
  import { messageText, isText, messageType, when, dayLabel, peerName, short } from './util.js';
  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';

  let { onunread } = $props();

  let conversations = $state([]);
  let connections = $state([]);
  let messages = $state([]);
  let loadingMessages = $state(false);
  let hasOlder = $state(false);
  let draft = $state('');
  let sending = $state(false);
  let search = $state('');
  let composing = $state(false);
  let newTarget = $state('');
  let scroller = $state();

  const PAGE = 60;
  const active = $derived(route.id);
  const activeConversation = $derived(conversations.find((c) => c.peer === active));
  const activeConnection = $derived(activeConversation?.connection ?? connections.find((c) => c.id === active || c.their_did === active));
  const activeProfile = $derived(activeConversation?.profile ?? activeConnection?.profile);
  const activeName = $derived(active ? peerName(active, { profile: activeProfile, connection: activeConnection }) : '');

  const filtered = $derived(
    conversations.filter((c) => {
      if (!search.trim()) return true;
      const q = search.toLowerCase();
      return peerName(c.peer, c).toLowerCase().includes(q) || messageText(c.last).toLowerCase().includes(q) || c.peer.toLowerCase().includes(q);
    }),
  );

  // Messages grouped by day, then into runs from the same side.
  const days = $derived.by(() => {
    const out = [];
    for (const m of messages) {
      const label = dayLabel(m.at);
      let day = out[out.length - 1];
      if (!day || day.label !== label) out.push((day = { label, runs: [] }));
      let run = day.runs[day.runs.length - 1];
      if (!run || run.direction !== m.direction || m.at - run.items[run.items.length - 1].at > 300) day.runs.push((run = { direction: m.direction, items: [] }));
      run.items.push(m);
    }
    return out;
  });

  async function loadConversations() {
    try {
      [conversations, connections] = await Promise.all([api.conversations(), api.connections()]);
      onunread?.(conversations.reduce((n, c) => n + (c.peer === active ? 0 : c.unread), 0));
    } catch (e) {
      if (e.status !== 401) toast(e.message, 'error');
    }
  }

  async function openPeer(peer) {
    messages = [];
    hasOlder = false;
    if (!peer) return;
    loadingMessages = true;
    try {
      const page = await api.messages(peer, { limit: PAGE });
      if (route.id !== peer) return;
      messages = page;
      hasOlder = page.length === PAGE;
      await scrollToBottom();
      markRead();
    } catch (e) {
      toast(e.message, 'error');
    } finally {
      loadingMessages = false;
    }
  }

  async function loadOlder() {
    if (!messages.length) return;
    const before = messages[0].id;
    const height = scroller.scrollHeight;
    const page = await api.messages(active, { before, limit: PAGE });
    messages = [...page, ...messages];
    hasOlder = page.length === PAGE;
    await tick();
    scroller.scrollTop = scroller.scrollHeight - height;
  }

  async function poll() {
    if (document.hidden) return;
    await loadConversations();
    if (!active) return;
    const after = messages.length ? messages[messages.length - 1].id : undefined;
    const atBottom = scroller && scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
    try {
      const fresh = after === undefined ? await api.messages(active, { limit: PAGE }) : await api.messages(active, { after });
      const known = new Set(messages.map((m) => m.id));
      const added = fresh.filter((m) => !known.has(m.id));
      if (added.length) {
        messages = [...messages, ...added];
        if (atBottom) await scrollToBottom();
        markRead();
      }
    } catch {
      /* next round */
    }
  }

  function markRead() {
    const last = messages[messages.length - 1];
    if (last && active) api.markRead(active, last.id).then(loadConversations).catch(() => {});
  }

  async function scrollToBottom() {
    await tick();
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }

  async function send(e) {
    e?.preventDefault();
    const content = draft.trim();
    if (!content || !active || sending) return;
    sending = true;
    try {
      const result = await api.send(active, content);
      draft = '';
      await poll();
      await scrollToBottom();
      if (result?.note && !/fetch_messages/.test(result.note)) toast(result.note);
    } catch (err) {
      toast(err.message, 'error');
    } finally {
      sending = false;
    }
  }

  function keydown(e) {
    if (e.key === 'Enter' && !e.shiftKey) send(e);
  }

  async function startChat(e) {
    e.preventDefault();
    const target = newTarget.trim();
    if (!target) return;
    composing = false;
    newTarget = '';
    go('chats', target);
  }

  async function refreshNow() {
    try {
      const r = await api.refresh();
      if (r.problems?.length) toast(r.problems[0], 'error');
      await poll();
    } catch (e) {
      toast(e.message, 'error');
    }
  }

  $effect(() => {
    openPeer(active);
  });

  onMount(() => {
    loadConversations();
    const timer = setInterval(poll, 4000);
    return () => clearInterval(timer);
  });
</script>

<div class="chats" class:open={!!active}>
  <aside class="list">
    <header>
      <h2>Chats</h2>
      <div class="actions">
        <button class="icon-btn" title="Check for new messages" onclick={refreshNow}><Icon name="refresh" size={18} /></button>
        <button class="icon-btn" title="New chat" onclick={() => (composing = !composing)}><Icon name="plus" size={18} /></button>
      </div>
    </header>
    {#if composing}
      <form class="compose" onsubmit={startChat}>
        <select class="input" bind:value={newTarget}>
          <option value="">Choose a connection…</option>
          {#each connections as c (c.id)}
            <option value={c.id}>{peerName(c.id, { profile: c.profile, connection: c })} · DIDComm {c.didcomm_version}</option>
          {/each}
        </select>
        <input class="input mono" placeholder="…or paste a DID" bind:value={newTarget} />
        <button class="btn primary small" disabled={!newTarget.trim()}>Open chat</button>
      </form>
    {/if}
    <div class="search">
      <Icon name="search" size={16} />
      <input placeholder="Search" bind:value={search} />
    </div>
    <ul class="scroll-thin">
      {#each filtered as c (c.peer)}
        {@const name = peerName(c.peer, c)}
        <li>
          <button class="row" class:active={c.peer === active} onclick={() => go('chats', c.peer)}>
            <Avatar {name} picture={c.profile?.displayPicture} seed={c.peer} size={46} />
            <div class="meta">
              <div class="top">
                <span class="name">{name}</span>
                <span class="time" class:unread={c.unread > 0 && c.peer !== active}>{when(c.last.at)}</span>
              </div>
              <div class="bottom">
                <span class="preview">{c.last.direction === 'out' ? 'You: ' : ''}{messageText(c.last)}</span>
                {#if c.unread > 0 && c.peer !== active}<b class="count">{c.unread}</b>{/if}
              </div>
            </div>
          </button>
        </li>
      {:else}
        <li class="empty">
          {#if search}No chats match “{search}”.{:else}No conversations yet.<br /><span class="muted">Start one with <b>+</b>, or share an invitation from Connections.</span>{/if}
        </li>
      {/each}
    </ul>
  </aside>

  <section class="thread">
    {#if !active}
      <div class="empty placeholder">
        <Icon name="chat" size={44} />
        <p>Select a chat, or start a new one.</p>
      </div>
    {:else}
      <header class="thread-head">
        <button class="icon-btn back" onclick={() => go('chats')} title="Back"><Icon name="back" /></button>
        <Avatar name={activeName} picture={activeProfile?.displayPicture} seed={active} size={40} />
        <div class="who">
          <div class="name">{activeName}</div>
          <div class="sub muted mono" title={activeConnection?.their_did || active}>
            {#if activeConnection}DIDComm {activeConnection.didcomm_version} · {/if}{short(activeConnection?.their_did || active, 8)}
          </div>
        </div>
        <button class="btn small ghost" title="Send them your profile and ask for theirs" onclick={() => api.shareProfile(active).then(() => toast('Profile sent')).catch((e) => toast(e.message, 'error'))}>
          <Icon name="share" size={16} /> Share profile
        </button>
      </header>
      {#if activeProfile?.description}
        <div class="bio muted">{activeProfile.description}</div>
      {/if}
      <div class="messages scroll-thin" bind:this={scroller}>
        {#if hasOlder}
          <button class="btn small older" onclick={loadOlder}>Load earlier messages</button>
        {/if}
        {#if loadingMessages}
          <div class="empty">Loading…</div>
        {:else if !messages.length}
          <div class="empty">No messages yet. Say hello 👋</div>
        {/if}
        {#each days as day (day.label)}
          <div class="day"><span>{day.label}</span></div>
          {#each day.runs as run}
            <div class="run {run.direction}">
              {#each run.items as m (m.id)}
                <div class="bubble" class:system={!isText(m)} title={messageType(m)}>
                  <span class="text">{messageText(m)}</span>
                  <span class="stamp">{when(m.at, true)}</span>
                </div>
              {/each}
            </div>
          {/each}
        {/each}
      </div>
      <form class="composer" onsubmit={send}>
        <textarea class="input" rows="1" placeholder="Message {activeName}" bind:value={draft} onkeydown={keydown}></textarea>
        <button class="send" disabled={!draft.trim() || sending} title="Send"><Icon name="send" size={18} /></button>
      </form>
    {/if}
  </section>
</div>

<style>
  .chats {
    display: grid;
    grid-template-columns: minmax(280px, 340px) 1fr;
    min-height: 0;
    height: 100%;
  }
  .list {
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    background: var(--panel);
    min-height: 0;
  }
  .list header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1rem 0.5rem;
  }
  h2 {
    margin: 0;
    font-size: 1.3rem;
  }
  .compose {
    display: grid;
    gap: 0.5rem;
    padding: 0 1rem 0.75rem;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0.25rem 1rem 0.5rem;
    padding: 0.45rem 0.75rem;
    border-radius: 10px;
    background: var(--panel-2);
    color: var(--muted);
  }
  .search input {
    border: none;
    background: none;
    outline: none;
    width: 100%;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0 0.5rem 1rem;
    overflow-y: auto;
    flex: 1;
  }
  .row {
    width: 100%;
    display: flex;
    gap: 0.75rem;
    align-items: center;
    padding: 0.65rem 0.6rem;
    border: none;
    background: none;
    border-radius: 12px;
    cursor: pointer;
    text-align: left;
  }
  .row:hover {
    background: var(--panel-2);
  }
  .row.active {
    background: var(--accent-soft);
  }
  .meta {
    min-width: 0;
    flex: 1;
    display: grid;
    gap: 0.15rem;
  }
  .top,
  .bottom {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .name {
    font-weight: 600;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .time {
    font-size: 0.75rem;
    color: var(--muted);
  }
  .time.unread {
    color: var(--accent);
    font-weight: 600;
  }
  .preview {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    font-size: 0.88rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    min-width: 20px;
    height: 20px;
    padding: 0 6px;
    border-radius: 10px;
    background: var(--accent-2);
    color: white;
    font-size: 0.72rem;
    display: grid;
    place-items: center;
  }
  .thread {
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
    background: var(--bg);
  }
  .placeholder {
    flex: 1;
  }
  .thread-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }
  .who {
    flex: 1;
    min-width: 0;
  }
  .who .sub {
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .back {
    display: none;
  }
  .bio {
    padding: 0.5rem 1rem;
    font-size: 0.85rem;
    background: var(--panel);
    border-bottom: 1px solid var(--border);
  }
  .messages {
    flex: 1;
    overflow-y: auto;
    padding: 1rem clamp(0.75rem, 3vw, 2rem);
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .older {
    align-self: center;
  }
  .day {
    align-self: center;
    margin: 0.5rem 0;
  }
  .day span {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted);
    background: var(--panel);
    border: 1px solid var(--border);
    padding: 0.2rem 0.7rem;
    border-radius: 999px;
  }
  .run {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    max-width: min(72%, 560px);
  }
  .run.out {
    align-self: flex-end;
    align-items: flex-end;
  }
  .run.in {
    align-self: flex-start;
  }
  .bubble {
    position: relative;
    padding: 0.5rem 0.8rem 0.5rem;
    border-radius: 18px;
    line-height: 1.4;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    display: flex;
    gap: 0.6rem;
    align-items: flex-end;
  }
  .in .bubble {
    background: var(--bubble-in);
    border: 1px solid var(--border);
    border-bottom-left-radius: 6px;
  }
  .out .bubble {
    background: var(--bubble-out);
    color: var(--bubble-out-text);
    border-bottom-right-radius: 6px;
  }
  .bubble.system {
    font-style: italic;
    opacity: 0.8;
  }
  .stamp {
    font-size: 0.68rem;
    opacity: 0.65;
    white-space: nowrap;
    flex: none;
  }
  .composer {
    display: flex;
    gap: 0.5rem;
    align-items: flex-end;
    padding: 0.75rem 1rem 1rem;
    background: var(--panel);
    border-top: 1px solid var(--border);
  }
  .composer textarea {
    resize: none;
    max-height: 9rem;
    field-sizing: content;
    border-radius: 20px;
    padding: 0.6rem 1rem;
  }
  .send {
    flex: none;
    width: 42px;
    height: 42px;
    border-radius: 50%;
    border: none;
    background: var(--accent-2);
    color: white;
    display: grid;
    place-items: center;
    cursor: pointer;
  }
  .send:disabled {
    opacity: 0.45;
    cursor: default;
  }
  @media (max-width: 720px) {
    .chats {
      grid-template-columns: 1fr;
    }
    .chats.open .list {
      display: none;
    }
    .chats:not(.open) .thread {
      display: none;
    }
    .back {
      display: inline-flex;
    }
    .run {
      max-width: 85%;
    }
  }
</style>
