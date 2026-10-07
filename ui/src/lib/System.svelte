<script>
  import { onMount } from 'svelte';
  import { api } from './api.js';
  import { toast } from './toast.svelte.js';
  import { duration, short } from './util.js';
  import Icon from './Icon.svelte';

  let status = $state(null);
  let config = $state(null);
  let identity = $state(null);
  let healthz = $state(null);

  async function load() {
    try {
      [status, config, identity] = await Promise.all([api.status(), api.config(), api.identity()]);
      healthz = await fetch('/healthz').then((r) => r.ok).catch(() => false);
    } catch (e) {
      toast(e.message, 'error');
    }
  }

  onMount(() => {
    load();
    const t = setInterval(load, 10000);
    return () => clearInterval(t);
  });

  const rows = $derived(
    config
      ? [
          ['Public URL', config.public_url],
          ['DIDComm endpoint', config.endpoint],
          ['DID method', config.did_method],
          ['Mediator (v2)', config.mediator_did ?? 'none'],
          ['Mediator (v1)', config.v1_mediator ?? 'none'],
          ['Registry', config.registry_did ?? 'disabled'],
          ['Validate messages', config.validate_messages ? 'yes' : 'no'],
          ['Allowed targets', config.allowed_targets?.join(', ') ?? 'any'],
          ['Storage', config.storage],
          ['Database', config.database ?? '—'],
          ['HTTP bind', config.http.bind],
          ['Allowed hosts', config.http.allowed_hosts?.join(', ') ?? 'loopback names'],
          ['Access token', config.http.auth_token ?? 'not set'],
        ]
      : [],
  );
</script>

<div class="page scroll-thin">
  <div class="head">
    <div>
      <h1>System</h1>
      <p class="sub muted">Health, status and configuration. Settings come from environment variables; change them in your host (e.g. Railway) and redeploy.</p>
    </div>
    <button class="btn" onclick={load}><Icon name="refresh" size={16} /> Refresh</button>
  </div>

  {#if !status}
    <div class="empty">Loading…</div>
  {:else}
    <div class="tiles">
      <div class="card tile">
        <span class="label muted">Server</span>
        <span class="value"><span class="dot" style="background:{healthz ? 'var(--ok)' : 'var(--bad)'}"></span> {healthz ? 'Healthy' : 'Unreachable'}</span>
        <span class="muted small">v{status.version} · up {duration(status.uptime_seconds)}</span>
      </div>
      <div class="card tile">
        <span class="label muted">Storage</span>
        <span class="value"><span class="dot" style="background:{status.storage.health.ok ? 'var(--ok)' : 'var(--bad)'}"></span> {status.storage.kind}</span>
        <span class="muted small">{status.storage.health.ok ? `${status.storage.inbox ?? 0} waiting for fetch_messages` : status.storage.health.error}</span>
      </div>
      <div class="card tile">
        <span class="label muted">Reachability</span>
        <span class="value">
          <span class="dot" style="background:{identity?.can_receive ? 'var(--ok)' : 'var(--warn)'}"></span>
          {identity?.can_receive ? (status.mediation ? 'Via mediator' : 'Own endpoint') : 'Replies only'}
        </span>
        <span class="muted small mono">{status.mediation ? short(status.mediation.mediator_did, 10) : (config?.endpoint ?? 'no endpoint')}</span>
      </div>
      <div class="card tile">
        <span class="label muted">Connections</span>
        <span class="value">{status.connections}</span>
        <span class="muted small">DIDComm v1 {status.v1_mediation ? '· v1 mediated' : ''}</span>
      </div>
    </div>

    <div class="section-title">Configuration</div>
    <div class="card table">
      {#each rows as [k, v]}
        <div class="tr"><span class="muted">{k}</span><span class="mono">{v ?? '—'}</span></div>
      {/each}
    </div>

    <div class="section-title">Identity</div>
    <div class="card table">
      <div class="tr"><span class="muted">DID</span><span class="mono">{identity?.did}</span></div>
      <div class="tr"><span class="muted">DIDComm v1 DID</span><span class="mono">{identity?.didcomm_v1?.did}</span></div>
      <div class="tr"><span class="muted">v1 verkey</span><span class="mono">{identity?.didcomm_v1?.verkey}</span></div>
    </div>
  {/if}
</div>

<style>
  .head {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    align-items: flex-start;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(210px, 1fr));
    gap: 1rem;
  }
  .tile {
    padding: 1.1rem 1.2rem;
    display: grid;
    gap: 0.3rem;
  }
  .label {
    font-size: 0.78rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .value {
    font-size: 1.25rem;
    font-weight: 650;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    text-transform: capitalize;
  }
  .small {
    font-size: 0.8rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .table {
    overflow: hidden;
  }
  .tr {
    display: grid;
    grid-template-columns: 200px 1fr;
    gap: 1rem;
    padding: 0.65rem 1.1rem;
    border-bottom: 1px solid var(--border);
    font-size: 0.9rem;
  }
  .tr:last-child {
    border-bottom: none;
  }
  .tr .mono {
    overflow-wrap: anywhere;
  }
  @media (max-width: 600px) {
    .tr {
      grid-template-columns: 1fr;
      gap: 0.2rem;
    }
  }
</style>
