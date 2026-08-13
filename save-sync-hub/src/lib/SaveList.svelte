<script lang="ts">
  import { convertFileSrc } from "@tauri-apps/api/core";

  let {
    saves = [],
    message,
    onBackup,
    onDownload,
    onRestore,
  }: {
    saves: any[];
    message: string;
    onBackup: (titleId: string, allPaths: string[], restoreDir: string) => void;
    onDownload: (titleId: string) => void;
    onRestore: (titleId: string, restoreDir: string) => void;
  } = $props();

  let grouped = $derived.by(() => {
    const groups: Record<string, any[]> = {};
    for (const s of saves) {
      const label = s.source_label || "Saves";
      if (!groups[label]) groups[label] = [];
      groups[label].push(s);
    }
    return Object.entries(groups);
  });

  function statusLabel(s: string): string {
    switch (s) {
      case "synced": return "Synced";
      case "upload": return "Upload needed";
      case "download": return "Download available";
      case "conflict": return "Conflict";
      case "local_only": return "Local only";
      case "cloud_only": return "Cloud only";
      default: return s;
    }
  }

  function statusColor(s: string): string {
    switch (s) {
      case "synced": return "#4caf50";
      case "upload": return "#00b4d8";
      case "download": return "#ff7700";
      case "conflict": return "#f44336";
      default: return "#666";
    }
  }

  function formatSize(bytes: number): string {
    if (bytes < 1024) return bytes + " B";
    if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
    return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  }

  function iconUrl(path: string | null | undefined): string | null {
    if (!path) return null;
    try { return convertFileSrc(path); } catch { return null; }
  }
</script>

<main class="content">
  <div class="header">
    Local Saves
    {#if saves.length > 0}
      <span class="count">{saves.length} entries</span>
    {/if}
  </div>

  {#if message}
    <div class="message" class:error={message.startsWith("Upload failed") || message.startsWith("Error")}>{message}</div>
  {/if}

  {#if saves.length === 0}
    <div class="empty">No saves found. Add a source and click Scan All.</div>
  {:else}
    {#each grouped as [label, entries]}
      <div class="group">
        <div class="group-header">{label}</div>
        <div class="list">
          {#each entries as save}
            <div class="row">
              {#if save.icon_path}
                <div class="icon-wrap">
                  <img src={iconUrl(save.icon_path)} alt="" class="icon" />
                </div>
              {/if}
              <div class="info">
                <span class="name">{save.name}</span>
                <span class="meta">
                  {formatSize(save.size)} · {save.timestamp}
                </span>
              </div>
              <div class="status">
                <span class="dot" style="background: {statusColor(save.status)}"></span>
                {statusLabel(save.status)}
              </div>
              <div class="actions">
                <button class="upload" onclick={() => onBackup(save.title_id, save.all_paths || [save.source_path], save.restore_dir || save.source_path || "")}>Upload</button>
                <button class="download" onclick={() => onDownload(save.title_id)}>Download</button>
                <button class="restore" onclick={() => onRestore(save.title_id, save.restore_dir || save.source_path || "")}>Restore</button>
              </div>
            </div>
          {/each}
        </div>
      </div>
    {/each}
  {/if}
</main>

<style>
  .content {
    flex: 1;
    padding: 20px 24px;
    overflow-y: auto;
  }
  .header {
    font-size: 1rem;
    font-weight: 600;
    margin-bottom: 12px;
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .count {
    font-size: 0.75rem;
    color: #666;
    font-weight: 400;
  }
  .message {
    background: #2a2a2a;
    color: #00b4d8;
    padding: 8px 12px;
    border-radius: 4px;
    margin-bottom: 12px;
    font-size: 0.85rem;
  }
  .message.error {
    color: #f44336;
    background: #2a1a1a;
  }
  .empty {
    color: #666;
    margin-top: 40px;
    text-align: center;
  }
  .group {
    margin-bottom: 16px;
  }
  .group-header {
    font-size: 0.75rem;
    color: #888;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 4px;
    padding-left: 2px;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 12px;
    background: #222;
    border-radius: 4px;
  }
  .row:hover {
    background: #2a2a2a;
  }
  .icon-wrap {
    width: 48px;
    min-width: 48px;
    height: 32px;
    border-radius: 2px;
    overflow: hidden;
    background: #111;
  }
  .icon {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .name {
    font-weight: 500;
    font-size: 0.9rem;
  }
  .meta {
    font-size: 0.75rem;
    color: #888;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8rem;
    color: #aaa;
    white-space: nowrap;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }
  .actions {
    display: flex;
    gap: 6px;
  }
  .actions button {
    padding: 4px 10px;
    border: 1px solid #444;
    background: transparent;
    color: #ccc;
    border-radius: 3px;
    font-size: 0.75rem;
    cursor: pointer;
    white-space: nowrap;
  }
  .actions button:hover {
    background: #333;
  }
  .upload { border-color: #00b4d8 !important; color: #00b4d8 !important; }
  .download { border-color: #ff7700 !important; color: #ff7700 !important; }
  .restore { border-color: #4caf50 !important; color: #4caf50 !important; }
</style>
