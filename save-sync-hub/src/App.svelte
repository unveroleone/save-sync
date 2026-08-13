<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { open } from "@tauri-apps/plugin-dialog";
  import Sidebar from "./lib/Sidebar.svelte";
  import SaveList from "./lib/SaveList.svelte";
  import CloudList from "./lib/CloudList.svelte";
  import Settings from "./lib/Settings.svelte";

  interface Source {
    id: string;
    platform: string;
    customPath: string;
    label: string;
    resolvedPath: string;
  }

  let sources = $state<Source[]>([
    { id: "psp", platform: "psp", customPath: "", label: "PSP (PPSSPP)", resolvedPath: "" },
    { id: "retroarch", platform: "retroarch", customPath: "", label: "RetroArch", resolvedPath: "" },
  ]);

  let saves = $state<any[]>([]);
  let cloudEntries = $state<any[]>([]);
  let devices = $state<any[]>([]);
  let loading = $state(false);
  let cloudLoading = $state(false);
  let deviceLoading = $state(false);
  let message = $state("");

  let serverUrl = $state("");
  let apiToken = $state("");
  let deviceName = $state("");

  let activeTab = $state("local");
  let customCounter = $state(0);

  async function loadConfig() {
    try {
      const c = await invoke("load_config_cmd");
      serverUrl = c.server_url;
      apiToken = c.api_token;
      deviceName = c.device_name;
      if (c.sources && c.sources.length > 0) {
        // Merge persisted sources with defaults.
        const merged: Source[] = [];
        const seen = new Set<string>();
        for (const s of c.sources) {
          merged.push({ id: s.id, platform: s.platform, customPath: s.custom_path ?? s.customPath ?? "", label: s.label, resolvedPath: "" });
          seen.add(s.id);
        }
        for (const def of defaultSources()) {
          if (!seen.has(def.id)) merged.push(def);
        }
        sources = merged;
      }
    } catch (e) {
      console.error("Load config failed:", e);
    }
  }

  function defaultSources(): Source[] {
    return [
      { id: "psp", platform: "psp", customPath: "", label: "PSP (PPSSPP)", resolvedPath: "" },
      { id: "retroarch", platform: "retroarch", customPath: "", label: "RetroArch", resolvedPath: "" },
    ];
  }

  async function saveSources() {
    try {
      const c = await invoke("load_config_cmd");
      c.sources = sources.map(s => ({
        id: s.id,
        platform: s.platform,
        customPath: s.customPath,
        label: s.label,
      }));
      await invoke("save_config_cmd", { config: c });
    } catch (e) {
      console.error("Save sources failed:", e);
    }
  }

  async function resolveSource(src: Source) {
    if (src.customPath) {
      src.resolvedPath = src.customPath;
      return;
    }
    try {
      src.resolvedPath = await invoke("resolve_platform_path", {
        platform: src.platform,
        customPath: src.platform === "custom" ? src.customPath : null,
      });
    } catch {
      src.resolvedPath = "Not found";
    }
  }

  async function initPaths() {
    for (const src of sources) {
      await resolveSource(src);
    }
  }

  async function browseSource(id: string) {
    const selected = await open({ directory: true, multiple: false, title: "Select save folder" });
    if (!selected || typeof selected !== "string") return;
    const src = sources.find(s => s.id === id);
    if (!src) return;
    src.customPath = selected;
    src.resolvedPath = selected;
    saveSources();
  }

  async function removeSource(id: string) {
    sources = sources.filter(s => s.id !== id);
    saveSources();
  }

  function renameSource(id: string, label: string) {
    const src = sources.find(s => s.id === id);
    if (!src) return;
    src.label = label;
    saveSources();
  }

  function addCustom() {
    customCounter++;
    const id = `custom-${customCounter}`;
    sources = [...sources, {
      id,
      platform: "custom",
      customPath: "",
      label: `Custom #${customCounter}`,
      resolvedPath: "",
    }];
    saveSources();
  }

  async function scanAll() {
    loading = true;
    message = "";
    const all: any[] = [];
    try {
      for (const src of sources) {
        const results = await invoke("scan_saves", {
          platform: src.platform,
          customPath: src.customPath || (src.platform === "custom" ? src.resolvedPath : null),
          sourceLabel: src.label,
        });
        all.push(...results);
      }
      saves = all;
      if (all.length === 0) message = "No saves found.";
    } catch (e: any) {
      message = "Error: " + e;
    } finally {
      loading = false;
    }
  }

  async function handleSaveConfig() {
    try {
      await invoke("save_config_cmd", {
        config: { server_url: serverUrl, api_token: apiToken, device_name: deviceName },
      });
      message = "Config saved.";
      setTimeout(() => (message = ""), 2000);
    } catch (e: any) {
      message = "Error: " + e;
    }
  }

  async function handleBackup(titleId: string, allPaths: string[], restoreDir: string) {
    message = "Uploading...";
    try {
      const res = await invoke("backup_and_upload", { titleId, allPaths, zipRoot: restoreDir });
      await scanAll();
      message = res as string;
    } catch (e: any) {
      message = "Upload failed: " + e;
    }
  }

  async function handleDownload(titleId: string) {
    message = "Downloading...";
    try {
      const res = await invoke("download_only", { titleId });
      message = "Downloaded to: " + res;
    } catch (e: any) {
      message = "Error: " + e;
    }
  }

  async function handleRestore(titleId: string, restoreDir: string) {
    message = "Downloading & restoring...";
    try {
      const res = await invoke("download_and_restore", { titleId, restoreDir });
      message = res as string;
    } catch (e: any) {
      message = "Error: " + e;
    }
  }

  async function loadCloudSaves() {
    cloudLoading = true;
    message = "";
    try {
      const paths = sources.map(s => s.resolvedPath).filter(Boolean);
      cloudEntries = await invoke("get_cloud_saves", { searchPaths: paths });
    } catch (e: any) {
      message = "Error: " + e;
    } finally {
      cloudLoading = false;
    }
  }

  async function handleCloudDownload(titleId: string) {
    message = "Downloading...";
    try {
      const res = await invoke("download_only", { titleId });
      message = "Downloaded to: " + res;
    } catch (e: any) {
      message = "Error: " + e;
    }
  }

  async function handleCloudRestore(titleId: string) {
    try {
      const target = sources[0]?.resolvedPath || "";
      if (!target) { message = "Error: no source path to restore to"; return; }
      message = "Downloading & restoring...";
      const res = await invoke("download_and_restore", { titleId, restoreDir: target });
      message = res as string;
    } catch (e: any) {
      message = "Error: " + e;
    }
  }

  async function handleCloudDelete(titleId: string) {
    message = "Deleting...";
    try {
      await invoke("delete_cloud_save", { titleId });
      await loadCloudSaves();
      message = `Deleted ${titleId}.`;
    } catch (e: any) {
      message = "Delete failed: " + e;
    }
  }

  async function loadDevices() {
    deviceLoading = true;
    try {
      devices = await invoke("get_devices");
    } catch (e: any) {
      console.error("Load devices failed:", e);
      devices = [];
    } finally {
      deviceLoading = false;
    }
  }

  loadConfig().then(() => initPaths());
</script>

<div class="app">
  <Sidebar
    bind:sources={sources}
    {loading}
    onBrowseSource={browseSource}
    onRemoveSource={removeSource}
    onRenameSource={renameSource}
    onAddCustom={addCustom}
    onScanAll={scanAll}
  />
  <div class="main-panel">
    <nav class="tabs">
      <button class="tab" class:active={activeTab === "local"} onclick={() => activeTab = "local"}>Local</button>
      <button class="tab" class:active={activeTab === "cloud"} onclick={() => { activeTab = "cloud"; loadCloudSaves(); }}>Cloud</button>
      <button class="tab" class:active={activeTab === "settings"} onclick={() => { activeTab = "settings"; loadDevices(); }}>Settings</button>
    </nav>

    {#if activeTab === "local"}
      <SaveList
        {saves}
        {message}
        onBackup={handleBackup}
        onDownload={handleDownload}
        onRestore={(titleId: string, restoreDir: string) => { handleRestore(titleId, restoreDir); }}
      />
    {:else if activeTab === "cloud"}
      <CloudList
        entries={cloudEntries}
        loading={cloudLoading}
        {message}
        onRefresh={loadCloudSaves}
        onDownload={handleCloudDownload}
        onRestore={handleCloudRestore}
        onDelete={handleCloudDelete}
      />
    {:else}
      <Settings
        bind:serverUrl={serverUrl}
        bind:apiToken={apiToken}
        bind:deviceName={deviceName}
        {devices}
        loading={deviceLoading}
        {message}
        onSaveConfig={handleSaveConfig}
        onRefreshDevices={loadDevices}
      />
    {/if}
  </div>
</div>

<style>
  .app { display: flex; height: 100vh; }
  .main-panel { flex: 1; display: flex; flex-direction: column; min-width: 0; }
  .tabs {
    display: flex;
    border-bottom: 1px solid #333;
    padding: 0 24px;
    background: #1a1a1a;
  }
  .tab {
    padding: 10px 20px;
    border: none;
    background: transparent;
    color: #888;
    font-size: 0.85rem;
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .tab:hover { color: #ccc; }
  .tab.active { color: #00b4d8; border-bottom-color: #00b4d8; }
</style>
