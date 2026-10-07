<script>
  import { onMount } from 'svelte';
  import { api, setUnauthorizedHandler } from './lib/api.js';
  import { route, go } from './lib/router.svelte.js';
  import { toasts } from './lib/toast.svelte.js';
  import Login from './lib/Login.svelte';
  import Chats from './lib/Chats.svelte';
  import Connections from './lib/Connections.svelte';
  import Profile from './lib/Profile.svelte';
  import System from './lib/System.svelte';
  import Icon from './lib/Icon.svelte';
  import Avatar from './lib/Avatar.svelte';

  let phase = $state('loading'); // loading | signed-out | ready
  let auth = $state(true);
  let me = $state({ profile: {}, identity: null });
  let unread = $state(0);

  setUnauthorizedHandler(() => (phase = 'signed-out'));

  async function start() {
    try {
      const s = await api.session();
      auth = s.auth;
      if (!s.signed_in) {
        phase = 'signed-out';
        return;
      }
      phase = 'ready';
      loadMe();
    } catch {
      phase = 'signed-out';
    }
  }

  async function loadMe() {
    const [profile, identity] = await Promise.all([api.profile(), api.identity()]);
    me = { profile, identity };
  }

  async function signOut() {
    await api.logout().catch(() => {});
    phase = 'signed-out';
  }

  onMount(start);

  const nav = [
    { page: 'chats', label: 'Chats', icon: 'chat' },
    { page: 'connections', label: 'Connections', icon: 'users' },
    { page: 'profile', label: 'Profile', icon: 'user' },
    { page: 'system', label: 'System', icon: 'activity' },
  ];
</script>

{#if phase === 'loading'}
  <div class="loading"><span class="spinner"></span></div>
{:else if phase === 'signed-out'}
  <Login onsignedin={start} />
{:else}
  <div class="shell" class:in-chat={route.page === 'chats' && route.id}>
    <nav class="rail">
      <div class="brand" title="didcomm-mcp"><Icon name="chat" size={20} /></div>
      {#each nav as item (item.page)}
        <button class="nav-item" class:active={route.page === item.page} onclick={() => go(item.page)} title={item.label}>
          <Icon name={item.icon} />
          <span>{item.label}</span>
          {#if item.page === 'chats' && unread > 0}<b class="badge">{unread > 99 ? '99+' : unread}</b>{/if}
        </button>
      {/each}
      <div class="spacer"></div>
      <button class="me" onclick={() => go('profile')} title="Your profile">
        <Avatar name={me.profile.displayName || 'Me'} picture={me.profile.displayPicture} seed={me.identity?.did} size={34} />
      </button>
      {#if auth}
        <button class="nav-item" onclick={signOut} title="Sign out"><Icon name="logout" /><span>Sign out</span></button>
      {/if}
    </nav>
    <main>
      {#if route.page === 'chats'}
        <Chats onunread={(n) => (unread = n)} />
      {:else if route.page === 'connections'}
        <Connections />
      {:else if route.page === 'profile'}
        <Profile identity={me.identity} onsaved={loadMe} />
      {:else if route.page === 'system'}
        <System />
      {:else}
        <div class="empty">Not found.</div>
      {/if}
    </main>
  </div>
{/if}

<div class="toasts">
  {#each toasts as t (t.id)}
    <div class="toast {t.kind}">{t.text}</div>
  {/each}
</div>

<style>
  .loading {
    height: 100%;
    display: grid;
    place-items: center;
  }
  .spinner {
    width: 28px;
    height: 28px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .shell {
    height: 100%;
    display: grid;
    grid-template-columns: 76px 1fr;
  }
  .rail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.35rem;
    padding: 0.9rem 0.5rem;
    background: var(--panel);
    border-right: 1px solid var(--border);
  }
  .brand {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    background: var(--accent-2);
    color: white;
    display: grid;
    place-items: center;
    margin-bottom: 0.9rem;
  }
  .nav-item {
    position: relative;
    width: 60px;
    border: none;
    background: transparent;
    border-radius: 12px;
    padding: 0.5rem 0.25rem;
    display: grid;
    justify-items: center;
    gap: 0.2rem;
    cursor: pointer;
    color: var(--muted);
    font-size: 0.68rem;
    font-weight: 600;
  }
  .nav-item:hover {
    background: var(--panel-2);
    color: var(--text);
  }
  .nav-item.active {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .badge {
    position: absolute;
    top: 2px;
    right: 6px;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 9px;
    background: var(--bad);
    color: white;
    font-size: 0.66rem;
    display: grid;
    place-items: center;
  }
  .spacer {
    flex: 1;
  }
  .me {
    border: none;
    background: none;
    padding: 0;
    cursor: pointer;
    border-radius: 50%;
    margin-bottom: 0.3rem;
  }
  main {
    min-width: 0;
    min-height: 0;
    display: grid;
  }
  .toasts {
    position: fixed;
    bottom: 1rem;
    right: 1rem;
    display: grid;
    gap: 0.5rem;
    z-index: 50;
  }
  .toast {
    background: var(--text);
    color: var(--panel);
    padding: 0.6rem 0.9rem;
    border-radius: 10px;
    font-size: 0.9rem;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.18);
    max-width: 380px;
  }
  .toast.error {
    background: var(--bad);
    color: white;
  }
  @media (max-width: 720px) {
    .shell {
      grid-template-columns: 1fr;
      grid-template-rows: 1fr auto;
    }
    .rail {
      grid-row: 2;
      flex-direction: row;
      justify-content: space-around;
      border-right: none;
      border-top: 1px solid var(--border);
      padding: 0.3rem;
    }
    .brand,
    .spacer,
    .me {
      display: none;
    }
    .shell.in-chat .rail {
      display: none;
    }
  }
</style>
