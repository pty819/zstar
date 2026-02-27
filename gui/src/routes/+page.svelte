<script>
  import { invoke } from "@tauri-apps/api/core";
  import { open, save } from "@tauri-apps/plugin-dialog";

  let currentView = $state('home'); // 'home', 'pack', 'unpack'

  // Pack state
  let sourcePath = $state("");
  let outputPath = $state("");
  let folderName = $state("");
  let folderSize = $state("");
  let level = $state(3);
  let threads = $state(navigator.hardwareConcurrency || 4);
  let ignoreFailedRead = $state(false);
  let noLong = $state(false);

  // Unpack state
  let archivePath = $state("");
  let archiveName = $state("");
  let unpackOutputPath = $state("");
  let unpackThreads = $state(navigator.hardwareConcurrency || 4);

  // Common state
  let isProcessing = $state(false);
  let progressText = $state("");
  let result = $state(null);
  let error = $state(null);

  let zstarExists = $state(false);
  let zstarPath = $state("");

  $effect(() => {
    checkZstar();
  });

  async function checkZstar() {
    try {
      const data = await invoke("check_zstar");
      zstarExists = data.exists;
      zstarPath = data.path;
    } catch (e) {
      error = "Cannot connect to backend: " + e;
    }
  }

  // ==================== PACK FUNCTIONS ====================
  async function selectFolder() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select folder to compress"
      });

      if (selected) {
        sourcePath = selected;
        await fetchFolderInfo();
      }
    } catch (e) {
      error = "Failed to select folder: " + e;
    }
  }

  async function fetchFolderInfo() {
    if (!sourcePath) return;

    try {
      const data = await invoke("get_folder_info", { path: sourcePath });
      folderName = data.name;
      folderSize = data.size;

      const separator = sourcePath.includes('\\') ? '\\' : '/';
      const basePath = sourcePath.substring(0, sourcePath.lastIndexOf(separator));
      outputPath = `${basePath}${separator}${folderName}.tar.zst`;
    } catch (e) {
      error = "Failed to get folder info: " + e;
    }
  }

  async function selectPackOutput() {
    try {
      const selected = await save({
        defaultPath: folderName ? `${folderName}.tar.zst` : "output.tar.zst",
        filters: [{ name: "Zstandard Archive", extensions: ["tar.zst", "zst"] }],
        title: "Select output location"
      });

      if (selected) {
        outputPath = selected;
      }
    } catch (e) {
      console.error("Error selecting output:", e);
    }
  }

  async function startPack() {
    if (!sourcePath || !outputPath) {
      error = "Please select source folder and output path";
      return;
    }

    error = null;
    result = null;
    isProcessing = true;
    progressText = "Compressing...";

    try {
      const data = await invoke("pack_folder", {
        sourcePath,
        outputPath,
        level: level || null,
        threads: threads || null,
        ignoreFailedRead: ignoreFailedRead || null,
        noLong: noLong || null
      });

      if (data.success) {
        result = data;
        progressText = "Complete!";
      } else {
        error = data.error || "Compression failed";
        progressText = "Failed";
      }
    } catch (e) {
      error = e.toString();
      progressText = "Error";
    } finally {
      isProcessing = false;
    }
  }

  // ==================== UNPACK FUNCTIONS ====================
  async function selectArchive() {
    try {
      const selected = await open({
        multiple: false,
        title: "Select archive to extract",
        filters: [{ name: "Zstandard Archive", extensions: ["tar.zst", "zst", "tar"] }]
      });

      if (selected) {
        archivePath = selected;
        const name = selected.split(/[/\\]/).pop();
        archiveName = name.replace(/\.(tar\.)?zst$/i, '');
        unpackOutputPath = selected.substring(0, Math.max(selected.lastIndexOf('/'), selected.lastIndexOf('\\'))) + '/' + archiveName;
      }
    } catch (e) {
      error = "Failed to select archive: " + e;
    }
  }

  async function selectUnpackOutput() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "Select output directory"
      });

      if (selected) {
        unpackOutputPath = selected;
      }
    } catch (e) {
      console.error("Error selecting output:", e);
    }
  }

  async function startUnpack() {
    if (!archivePath || !unpackOutputPath) {
      error = "Please select archive and output path";
      return;
    }

    error = null;
    result = null;
    isProcessing = true;
    progressText = "Extracting...";

    try {
      const data = await invoke("unpack_folder", {
        archivePath,
        outputPath: unpackOutputPath,
        threads: unpackThreads || null
      });

      if (data.success) {
        result = data;
        progressText = "Complete!";
      } else {
        error = data.error || "Extraction failed";
        progressText = "Failed";
      }
    } catch (e) {
      error = e.toString();
      progressText = "Error";
    } finally {
      isProcessing = false;
    }
  }

  // ==================== WINDOW CONTROLS ====================
  async function minimize() {
    await invoke("minimize_window");
  }

  async function maximize() {
    await invoke("maximize_window");
  }

  async function close() {
    await invoke("close_window");
  }

  function goHome() {
    currentView = 'home';
    resetStates();
  }

  function resetStates() {
    result = null;
    error = null;
    progressText = "";
  }
</script>

<main>
  <!-- Custom Title Bar -->
  <div class="titlebar" data-tauri-drag-region>
    <div class="titlebar-left">
      <span class="logo-icon">⚡</span>
      <span class="title">zstar</span>
    </div>
    <div class="titlebar-right">
      <button class="titlebar-btn" onclick={minimize}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M5 12h14"/></svg>
      </button>
      <button class="titlebar-btn" onclick={maximize}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><rect x="4" y="4" width="16" height="16" rx="2"/></svg>
      </button>
      <button class="titlebar-btn close" onclick={close}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M6 6l12 12M6 18L18 6"/></svg>
      </button>
    </div>
  </div>

  <!-- Home View -->
  {#if currentView === 'home'}
    <div class="home-container">
      <div class="home-content">
        <div class="home-logo">
          <span class="logo-large">⚡</span>
        </div>
        <h1 class="home-title">zstar</h1>
        <p class="home-subtitle">High-Performance Parallel Archiver</p>

        <div class="home-cards">
          <button class="home-card" onclick={() => currentView = 'pack'}>
            <div class="card-icon pack">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor">
                <path d="M21 8v8a2 2 0 01-2 2H5a2 2 0 01-2-2V8"/>
                <path d="M24 8V6a2 2 0 00-2-2H4a2 2 0 00-2 2v2"/>
                <path d="M12 12v4"/>
                <path d="M8 14v2"/>
                <path d="M16 14v2"/>
              </svg>
            </div>
            <span class="card-title">Compress</span>
            <span class="card-desc">Pack files into .tar.zst</span>
          </button>

          <button class="home-card" onclick={() => currentView = 'unpack'}>
            <div class="card-icon unpack">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor">
                <path d="M21 8v8a2 2 0 01-2 2H5a2 2 0 01-2-2V8"/>
                <path d="M3 8V6a2 2 0 012-2h14a2 2 0 012 2v2"/>
                <path d="M12 12v4"/>
                <path d="M8 12v2"/>
                <path d="M16 12v2"/>
              </svg>
            </div>
            <span class="card-title">Extract</span>
            <span class="card-desc">Unpack .tar.zst archives</span>
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Pack View -->
  {#if currentView === 'pack'}
    <div class="page-container">
      <button class="back-btn" onclick={goHome}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M19 12H5M12 19l-7-7 7-7"/></svg>
        Back
      </button>

      <div class="page-header">
        <div class="header-icon pack">📦</div>
        <h2>Compress Files</h2>
      </div>

      {#if !zstarExists}
        <div class="alert alert-error">
          <span>⚠️</span>
          <span>zstar.exe not found</span>
        </div>
      {/if}

      <div class="section">
        <label class="section-label">Source Folder</label>
        <button class="select-btn" onclick={selectFolder} disabled={isProcessing}>
          <span>📁</span>
          Select Folder
        </button>
        {#if folderName}
          <div class="selected-info">
            <div class="info-row">
              <span class="info-icon">📂</span>
              <div class="info-content">
                <span class="info-name">{folderName}</span>
                <span class="info-path">{sourcePath}</span>
              </div>
            </div>
            <span class="info-size">{folderSize}</span>
          </div>
        {/if}
      </div>

      <div class="section">
        <label class="section-label">Output</label>
        <div class="input-row">
          <input
            type="text"
            class="input"
            bind:value={outputPath}
            placeholder="Output path..."
            disabled={isProcessing}
          />
          <button class="btn-icon" onclick={selectPackOutput} disabled={isProcessing}>📂</button>
        </div>
      </div>

      <div class="section options">
        <div class="option-item">
          <label>Compression Level</label>
          <div class="slider-row">
            <input type="range" min="1" max="22" bind:value={level} disabled={isProcessing} class="slider"/>
            <span class="slider-value">{level}</span>
          </div>
        </div>

        <div class="option-item">
          <label>Threads</label>
          <div class="slider-row">
            <input type="range" min="1" max="32" bind:value={threads} disabled={isProcessing} class="slider"/>
            <span class="slider-value">{threads}</span>
          </div>
        </div>

        <div class="checkbox-row">
          <label class="checkbox">
            <input type="checkbox" bind:checked={ignoreFailedRead} disabled={isProcessing}/>
            <span class="checkmark"></span>
            Ignore errors
          </label>
          <label class="checkbox">
            <input type="checkbox" bind:checked={noLong} disabled={isProcessing}/>
            <span class="checkmark"></span>
            No long mode
          </label>
        </div>
      </div>

      <button
        class="action-btn"
        onclick={startPack}
        disabled={isProcessing || !sourcePath || !outputPath || !zstarExists}
      >
        {#if isProcessing}
          <span class="spinner"></span>
          Compressing...
        {:else}
          <span>🚀</span>
          Compress
        {/if}
      </button>

      {#if isProcessing}
        <div class="progress">
          <div class="progress-bar"></div>
          <span class="progress-text">{progressText}</span>
        </div>
      {/if}

      {#if error}
        <div class="alert alert-error">❌ {error}</div>
      {/if}

      {#if result}
        <div class="alert alert-success">
          ✅ Completed in {result.duration.toFixed(2)}s
        </div>
      {/if}
    </div>
  {/if}

  <!-- Unpack View -->
  {#if currentView === 'unpack'}
    <div class="page-container">
      <button class="back-btn" onclick={goHome}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"><path d="M19 12H5M12 19l-7-7 7-7"/></svg>
        Back
      </button>

      <div class="page-header">
        <div class="header-icon unpack">📂</div>
        <h2>Extract Archive</h2>
      </div>

      {#if !zstarExists}
        <div class="alert alert-error">
          <span>⚠️</span>
          <span>zstar.exe not found</span>
        </div>
      {/if}

      <div class="section">
        <label class="section-label">Archive File</label>
        <button class="select-btn" onclick={selectArchive} disabled={isProcessing}>
          <span>📄</span>
          Select Archive
        </button>
        {#if archiveName}
          <div class="selected-info">
            <div class="info-row">
              <span class="info-icon">📦</span>
              <div class="info-content">
                <span class="info-name">{archiveName}</span>
                <span class="info-path">{archivePath}</span>
              </div>
            </div>
          </div>
        {/if}
      </div>

      <div class="section">
        <label class="section-label">Output Directory</label>
        <div class="input-row">
          <input
            type="text"
            class="input"
            bind:value={unpackOutputPath}
            placeholder="Output directory..."
            disabled={isProcessing}
          />
          <button class="btn-icon" onclick={selectUnpackOutput} disabled={isProcessing}>📂</button>
        </div>
      </div>

      <div class="section options">
        <div class="option-item">
          <label>Threads</label>
          <div class="slider-row">
            <input type="range" min="1" max="32" bind:value={unpackThreads} disabled={isProcessing} class="slider"/>
            <span class="slider-value">{unpackThreads}</span>
          </div>
        </div>
      </div>

      <button
        class="action-btn"
        onclick={startUnpack}
        disabled={isProcessing || !archivePath || !unpackOutputPath || !zstarExists}
      >
        {#if isProcessing}
          <span class="spinner"></span>
          Extracting...
        {:else}
          <span>📤</span>
          Extract
        {/if}
      </button>

      {#if isProcessing}
        <div class="progress">
          <div class="progress-bar"></div>
          <span class="progress-text">{progressText}</span>
        </div>
      {/if}

      {#if error}
        <div class="alert alert-error">❌ {error}</div>
      {/if}

      {#if result}
        <div class="alert alert-success">
          ✅ Completed in {result.duration.toFixed(2)}s
        </div>
      {/if}
    </div>
  {/if}
</main>

<style>
  :global(*) {
    margin: 0;
    padding: 0;
    box-sizing: border-box;
  }

  :global(body) {
    font-family: 'Outfit', sans-serif;
    background: transparent;
    color: #f0f0f0;
    min-height: 100vh;
    overflow: hidden;
  }

  main {
    width: 100vw;
    height: 100vh;
    background: #0a0b0f;
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    overflow: hidden;
  }

  /* Title Bar */
  .titlebar {
    height: 40px;
    background: rgba(12, 14, 18, 0.95);
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0 12px;
    user-select: none;
    -webkit-app-region: drag;
    border-bottom: 1px solid rgba(255, 107, 53, 0.1);
  }

  .titlebar-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .logo-icon {
    font-size: 16px;
  }

  .title {
    font-family: 'JetBrains Mono', monospace;
    font-size: 13px;
    font-weight: 600;
    color: #ff6b35;
  }

  .titlebar-right {
    display: flex;
    gap: 4px;
    -webkit-app-region: no-drag;
  }

  .titlebar-btn {
    width: 32px;
    height: 28px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: #6b7280;
    cursor: pointer;
    transition: all 0.15s;
  }

  .titlebar-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: #e5e7eb;
  }

  .titlebar-btn.close:hover {
    background: #ef4444;
    color: white;
  }

  .titlebar-btn svg {
    width: 14px;
    height: 14px;
  }

  /* Home */
  .home-container {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    background:
      radial-gradient(ellipse at 30% 20%, rgba(255, 107, 53, 0.12) 0%, transparent 50%),
      radial-gradient(ellipse at 70% 80%, rgba(0, 212, 255, 0.08) 0%, transparent 50%);
  }

  .home-content {
    text-align: center;
    max-width: 400px;
  }

  .home-logo {
    margin-bottom: 16px;
  }

  .logo-large {
    font-size: 56px;
    display: inline-block;
    animation: pulse 2s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { transform: scale(1); }
    50% { transform: scale(1.05); }
  }

  .home-title {
    font-family: 'JetBrains Mono', monospace;
    font-size: 32px;
    font-weight: 700;
    color: #ff6b35;
    margin-bottom: 4px;
    letter-spacing: -1px;
  }

  .home-subtitle {
    font-size: 14px;
    color: #6b7280;
    margin-bottom: 32px;
  }

  .home-cards {
    display: flex;
    gap: 16px;
    justify-content: center;
  }

  .home-card {
    width: 140px;
    padding: 24px 16px;
    background: rgba(18, 20, 26, 0.8);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 16px;
    cursor: pointer;
    transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .home-card:hover {
    transform: translateY(-4px);
    border-color: rgba(255, 107, 53, 0.3);
    box-shadow: 0 12px 32px rgba(0, 0, 0, 0.4);
  }

  .card-icon {
    width: 48px;
    height: 48px;
    border-radius: 12px;
    display: flex;
    align-items: center;
    justify-content: center;
    margin-bottom: 4px;
  }

  .card-icon.pack {
    background: linear-gradient(135deg, #ff6b35 0%, #ff8c5a 100%);
  }

  .card-icon.unpack {
    background: linear-gradient(135deg, #00d4ff 0%, #38bdf8 100%);
  }

  .card-icon svg {
    width: 24px;
    height: 24px;
    color: white;
  }

  .card-title {
    font-weight: 600;
    font-size: 15px;
    color: #f0f0f0;
  }

  .card-desc {
    font-size: 11px;
    color: #6b7280;
  }

  /* Page Container */
  .page-container {
    flex: 1;
    padding: 20px 24px 24px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .back-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: transparent;
    border: none;
    color: #6b7280;
    font-size: 13px;
    cursor: pointer;
    border-radius: 6px;
    transition: all 0.15s;
    width: fit-content;
  }

  .back-btn:hover {
    background: rgba(255, 255, 255, 0.05);
    color: #e5e7eb;
  }

  .back-btn svg {
    width: 14px;
    height: 14px;
  }

  .page-header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 8px;
  }

  .header-icon {
    width: 40px;
    height: 40px;
    border-radius: 10px;
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 20px;
  }

  .header-icon.pack {
    background: linear-gradient(135deg, #ff6b35 0%, #ff8c5a 100%);
  }

  .header-icon.unpack {
    background: linear-gradient(135deg, #00d4ff 0%, #38bdf8 100%);
  }

  .page-header h2 {
    font-size: 18px;
    font-weight: 600;
  }

  /* Sections */
  .section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .section-label {
    font-size: 11px;
    font-weight: 600;
    color: #6b7280;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .select-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 14px;
    background: rgba(24, 26, 32, 0.8);
    border: 1px dashed rgba(255, 107, 53, 0.3);
    border-radius: 10px;
    color: #e5e7eb;
    font-size: 14px;
    cursor: pointer;
    transition: all 0.2s;
  }

  .select-btn:hover:not(:disabled) {
    border-color: #ff6b35;
    background: rgba(255, 107, 53, 0.08);
  }

  .select-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .selected-info {
    background: rgba(18, 20, 26, 0.9);
    border: 1px solid rgba(255, 107, 53, 0.15);
    border-radius: 10px;
    padding: 12px;
  }

  .info-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .info-icon {
    font-size: 24px;
  }

  .info-content {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .info-name {
    font-weight: 500;
    font-size: 14px;
  }

  .info-path {
    font-family: 'JetBrains Mono', monospace;
    font-size: 10px;
    color: #6b7280;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 320px;
  }

  .info-size {
    font-size: 13px;
    color: #ff6b35;
    margin-top: 6px;
  }

  /* Input */
  .input-row {
    display: flex;
    gap: 8px;
  }

  .input {
    flex: 1;
    padding: 12px 14px;
    background: rgba(18, 20, 26, 0.8);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 8px;
    color: #e5e7eb;
    font-family: 'JetBrains Mono', monospace;
    font-size: 12px;
  }

  .input:focus {
    outline: none;
    border-color: rgba(255, 107, 53, 0.5);
  }

  .input:disabled {
    opacity: 0.6;
  }

  .btn-icon {
    width: 44px;
    height: 44px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(24, 26, 32, 0.8);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: 8px;
    font-size: 18px;
    cursor: pointer;
    transition: all 0.15s;
  }

  .btn-icon:hover:not(:disabled) {
    border-color: #ff6b35;
  }

  .btn-icon:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Options */
  .section.options {
    background: rgba(18, 20, 26, 0.6);
    border-radius: 10px;
    padding: 14px;
    gap: 14px;
  }

  .option-item {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .option-item label {
    font-size: 12px;
    font-weight: 500;
    color: #9ca3af;
  }

  .slider-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .slider {
    flex: 1;
    -webkit-appearance: none;
    height: 4px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 2px;
    cursor: pointer;
  }

  .slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 14px;
    height: 14px;
    background: #ff6b35;
    border-radius: 50%;
    cursor: pointer;
    transition: transform 0.15s;
  }

  .slider::-webkit-slider-thumb:hover {
    transform: scale(1.2);
  }

  .slider-value {
    font-family: 'JetBrains Mono', monospace;
    font-size: 13px;
    font-weight: 600;
    color: #ff6b35;
    min-width: 28px;
    text-align: right;
  }

  .checkbox-row {
    display: flex;
    gap: 16px;
  }

  .checkbox {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: #9ca3af;
    cursor: pointer;
  }

  .checkbox input {
    display: none;
  }

  .checkmark {
    width: 16px;
    height: 16px;
    border: 1.5px solid rgba(255, 255, 255, 0.2);
    border-radius: 4px;
    position: relative;
    transition: all 0.15s;
  }

  .checkbox input:checked + .checkmark {
    background: #ff6b35;
    border-color: #ff6b35;
  }

  .checkbox input:checked + .checkmark::after {
    content: '✓';
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    color: white;
    font-size: 10px;
  }

  /* Action Button */
  .action-btn {
    width: 100%;
    padding: 14px;
    background: linear-gradient(135deg, #ff6b35 0%, #ff8c5a 100%);
    border: none;
    border-radius: 10px;
    color: white;
    font-size: 15px;
    font-weight: 600;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    transition: all 0.2s;
    margin-top: 8px;
  }

  .action-btn:hover:not(:disabled) {
    transform: translateY(-2px);
    box-shadow: 0 8px 24px rgba(255, 107, 53, 0.35);
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Progress */
  .progress {
    margin-top: 12px;
  }

  .progress-bar {
    height: 4px;
    background: rgba(255, 255, 255, 0.1);
    border-radius: 2px;
    overflow: hidden;
  }

  .progress-bar::after {
    content: '';
    display: block;
    height: 100%;
    width: 30%;
    background: linear-gradient(90deg, #ff6b35, #ff8c5a);
    animation: progress 1.2s ease-in-out infinite;
  }

  @keyframes progress {
    0% { transform: translateX(-100%); }
    100% { transform: translateX(400%); }
  }

  .progress-text {
    display: block;
    text-align: center;
    font-size: 12px;
    color: #6b7280;
    margin-top: 8px;
  }

  /* Alert */
  .alert {
    padding: 12px 14px;
    border-radius: 8px;
    font-size: 13px;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .alert-error {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.2);
    color: #fca5a5;
  }

  .alert-success {
    background: rgba(34, 197, 94, 0.1);
    border: 1px solid rgba(34, 197, 94, 0.2);
    color: #86efac;
  }

  .spinner {
    width: 16px;
    height: 16px;
    border: 2px solid rgba(255, 255, 255, 0.3);
    border-top-color: white;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
