const addressInput = document.getElementById("address");
const startBtn = document.getElementById("startBtn");
const stopBtn = document.getElementById("stopBtn");
const statusText = document.getElementById("statusText");
const logPath = document.getElementById("logPath");
const errorText = document.getElementById("errorText");
const logList = document.getElementById("logList");
const logEmpty = document.getElementById("logEmpty");
const changeLogDirBtn = document.getElementById("changeLogDirBtn");
const alertBtn = document.getElementById("alertBtn");
const alertModal = document.getElementById("alertModal");
const closeAlertBtn = document.getElementById("closeAlertBtn");
const smtpHost = document.getElementById("smtpHost");
const smtpPort = document.getElementById("smtpPort");
const smtpUser = document.getElementById("smtpUser");
const smtpPass = document.getElementById("smtpPass");
const smtpFrom = document.getElementById("smtpFrom");
const smtpTo = document.getElementById("smtpTo");
const smtpTlsMode = document.getElementById("smtpTlsMode");
const saveSmtpBtn = document.getElementById("saveSmtpBtn");
const exportAlertBtn = document.getElementById("exportAlertBtn");
const importAlertBtn = document.getElementById("importAlertBtn");
const testSmtpBtn = document.getElementById("testSmtpBtn");
const smtpStatus = document.getElementById("smtpStatus");
const scrollTopBtn = document.getElementById("scrollTopBtn");
const scrollBottomBtn = document.getElementById("scrollBottomBtn");

const tauri = window.__TAURI__;
const invoke = tauri && tauri.core && typeof tauri.core.invoke === "function" ? tauri.core.invoke : null;
const eventApi = tauri && tauri.event ? tauri.event : null;

let running = false;
document.body.dataset.running = "false";
const maxLogs = 100;
const logs = [];
let unlisten = null;
let pollTimer = null;
let autoScroll = true;
const autoScrollThreshold = 6;
let settingsCache = null;
let sendingTest = false;

function updateScrollButtons() {
  if (!logList || !scrollTopBtn || !scrollBottomBtn) {
    return;
  }
  const distanceToBottom =
    logList.scrollHeight - logList.clientHeight - logList.scrollTop;
  const atTop = logList.scrollTop <= autoScrollThreshold;
  const atBottom = distanceToBottom <= autoScrollThreshold;

  if (atTop) {
    scrollBottomBtn.hidden = false;
    scrollTopBtn.hidden = true;
  } else if (atBottom) {
    scrollTopBtn.hidden = false;
    scrollBottomBtn.hidden = true;
  } else {
    scrollTopBtn.hidden = false;
    scrollBottomBtn.hidden = false;
  }
}

function renderLogs() {
  if (!logList || !logEmpty) {
    return;
  }
  const prevScrollTop = logList.scrollTop;

  logList.textContent = "";
  if (logs.length === 0) {
    logEmpty.hidden = false;
    return;
  }
  logEmpty.hidden = true;
  const fragment = document.createDocumentFragment();
  logs.forEach((entry) => {
    const item = document.createElement("li");
    item.value = entry.seq;
    item.textContent = entry.line;
    fragment.appendChild(item);
  });
  logList.appendChild(fragment);

  if (autoScroll) {
    logList.scrollTop = logList.scrollHeight - logList.clientHeight;
  } else {
    logList.scrollTop = prevScrollTop;
  }
  updateScrollButtons();
}

function addLog(entry) {
  if (!entry || typeof entry.seq !== "number" || typeof entry.line !== "string") {
    return;
  }
  if (logs.length > 0 && logs[logs.length - 1].seq >= entry.seq) {
    return;
  }
  logs.push(entry);
  if (logs.length > maxLogs) {
    logs.shift();
  }
  renderLogs();
}

function logsEqual(next) {
  if (logs.length !== next.length) {
    return false;
  }
  for (let i = 0; i < logs.length; i += 1) {
    if (logs[i].seq !== next[i].seq || logs[i].line !== next[i].line) {
      return false;
    }
  }
  return true;
}

function normalizeEntries(raw) {
  if (!Array.isArray(raw)) {
    return [];
  }
  const entries = [];
  raw.forEach((item) => {
    if (!item || typeof item.seq !== "number" || typeof item.line !== "string") {
      return;
    }
    entries.push({ seq: item.seq, line: item.line });
  });
  return entries;
}

async function fetchLogs() {
  if (!invoke) {
    return;
  }
  try {
    const recent = await invoke("get_recent_logs");
    const entries = normalizeEntries(recent);
    if (entries.length === 0 && recent && Array.isArray(recent) && recent.length > 0) {
      return;
    }
    if (!entries.length && logs.length === 0) {
      renderLogs();
      return;
    }
    const sliced = entries.slice(0, maxLogs);
    if (logsEqual(sliced)) {
      return;
    }
    logs.length = 0;
    logs.push(...sliced);
    renderLogs();
  } catch {
    // Ignore polling errors; file logging continues.
  }
}

function setRunning(next) {
  running = next;
  startBtn.disabled = next;
  stopBtn.disabled = !next;
  addressInput.disabled = next;
  if (changeLogDirBtn) {
    changeLogDirBtn.disabled = next;
  }
  statusText.textContent = next
    ? `运行中：${addressInput.value.trim()}`
    : "空闲";
  document.body.dataset.running = next ? "true" : "false";
}

function setError(message) {
  errorText.textContent = message || "";
}

function setSmtpStatus(message, kind = "") {
  if (!smtpStatus) {
    return;
  }
  smtpStatus.textContent = message || "";
  smtpStatus.classList.remove("success", "error");
  if (kind) {
    smtpStatus.classList.add(kind);
  }
}

function setTestSending(next) {
  sendingTest = next;
  if (testSmtpBtn) {
    testSmtpBtn.disabled = next;
    testSmtpBtn.textContent = next ? "发送中..." : "发送测试邮件";
  }
  if (saveSmtpBtn) {
    saveSmtpBtn.disabled = next;
  }
}

function setActiveTab(name) {
  const tabs = document.querySelectorAll(".tab");
  const panels = document.querySelectorAll(".tab-panel");
  tabs.forEach((tab) => {
    tab.classList.toggle("active", tab.dataset.tab === name);
  });
  panels.forEach((panel) => {
    panel.classList.toggle("hidden", panel.dataset.panel !== name);
  });
}

function openAlertModal() {
  if (!alertModal) {
    return;
  }
  alertModal.classList.remove("hidden");
  alertModal.setAttribute("aria-hidden", "false");
  document.body.classList.add("modal-open");
  setActiveTab("email");
  setSmtpStatus("");
}

function closeAlertModal() {
  if (!alertModal) {
    return;
  }
  alertModal.classList.add("hidden");
  alertModal.setAttribute("aria-hidden", "true");
  document.body.classList.remove("modal-open");
}

function applySmtpSettings(settings) {
  if (!settings || !settings.smtp) {
    return;
  }
  const smtp = settings.smtp;
  if (smtpHost) smtpHost.value = smtp.host || "";
  if (smtpPort) smtpPort.value = smtp.port ? String(smtp.port) : "";
  if (smtpUser) smtpUser.value = smtp.username || "";
  if (smtpPass) smtpPass.value = smtp.password || "";
  if (smtpFrom) smtpFrom.value = smtp.from || "";
  if (smtpTo) smtpTo.value = smtp.to || "";
  if (smtpTlsMode) {
    smtpTlsMode.value = smtp.tls_mode || (smtp.use_tls ? "ssl" : "ssl");
  }
}

function collectSmtpSettings() {
  const tlsMode = smtpTlsMode ? smtpTlsMode.value : "ssl";
  const portValue = smtpPort ? Number(smtpPort.value) : 0;
  const fallbackPort = tlsMode === "starttls" ? 587 : 465;
  return {
    host: smtpHost ? smtpHost.value.trim() : "",
    port: Number.isFinite(portValue) && portValue > 0 ? portValue : fallbackPort,
    username: smtpUser ? smtpUser.value.trim() : "",
    password: smtpPass ? smtpPass.value : "",
    from: smtpFrom ? smtpFrom.value.trim() : "",
    to: smtpTo ? smtpTo.value.trim() : "",
    tls_mode: tlsMode,
    use_tls: false,
  };
}

async function loadAlertSettings() {
  if (!invoke) {
    return;
  }
  try {
    const settings = await invoke("get_alert_settings");
    settingsCache = settings || null;
    applySmtpSettings(settingsCache);
    setSmtpStatus("");
  } catch (err) {
    setSmtpStatus(String(err), "error");
  }
}

async function saveAlertSettings() {
  if (!invoke) {
    return;
  }
  const smtp = collectSmtpSettings();
  settingsCache = settingsCache || {};
  settingsCache.smtp = smtp;
  settingsCache.wechat = settingsCache.wechat || {};
  try {
    await invoke("save_alert_settings", { settings: settingsCache });
    setSmtpStatus("已保存配置。", "success");
  } catch (err) {
    setSmtpStatus(String(err), "error");
  }
}

async function testSmtp() {
  if (!invoke) {
    return;
  }
  if (sendingTest) {
    return;
  }
  const smtp = collectSmtpSettings();
  setTestSending(true);
  setSmtpStatus("发送中…");
  try {
    const result = await invoke("test_smtp", { smtp });
    setSmtpStatus(String(result || "发送成功。"), "success");
  } catch (err) {
    setSmtpStatus(String(err), "error");
  } finally {
    setTestSending(false);
  }
}

async function exportAlertSettings() {
  if (!invoke) {
    return;
  }
  setSmtpStatus("导出中…");
  try {
    const result = await invoke("export_alert_settings");
    if (!result) {
      setSmtpStatus("已取消导出。");
      return;
    }
    setSmtpStatus(`已导出到: ${result}`, "success");
  } catch (err) {
    setSmtpStatus(String(err), "error");
  }
}

async function importAlertSettings() {
  if (!invoke) {
    return;
  }
  setSmtpStatus("导入中…");
  try {
    const settings = await invoke("import_alert_settings");
    if (!settings) {
      setSmtpStatus("已取消导入。");
      return;
    }
    settingsCache = settings;
    applySmtpSettings(settingsCache);
    setSmtpStatus("导入成功。", "success");
  } catch (err) {
    setSmtpStatus(String(err), "error");
  }
}

async function refreshLogDir() {
  if (!invoke) {
    return;
  }
  try {
    const current = await invoke("get_log_dir");
    if (typeof current === "string" && logPath) {
      logPath.textContent = current;
    }
  } catch (err) {
    setError(String(err));
  }
}

async function changeLogDir() {
  if (!invoke || running) {
    return;
  }
  setError("");
  try {
    const current = await invoke("select_log_dir");
    if (typeof current === "string" && logPath) {
      logPath.textContent = current;
    }
  } catch (err) {
    setError(String(err));
  }
}

async function startPing() {
  const address = addressInput.value.trim();
  if (!address) {
    setError("请输入目标地址。");
    return;
  }

  setError("");

  try {
    const baseDir = await invoke("start_ping", { address });
    logPath.textContent = baseDir;
    logs.length = 0;
    renderLogs();
    autoScroll = true;
    setRunning(true);
    await fetchLogs();
    if (pollTimer) {
      clearInterval(pollTimer);
    }
    pollTimer = setInterval(fetchLogs, 1000);
  } catch (err) {
    setError(String(err));
  }
}

async function stopPing() {
  setError("");

  try {
    await invoke("stop_ping");
    setRunning(false);
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  } catch (err) {
    setError(String(err));
  }
}

if (!invoke) {
  statusText.textContent = "Tauri API 不可用，请在应用内运行。";
  startBtn.disabled = true;
  stopBtn.disabled = true;
  addressInput.disabled = true;
  if (changeLogDirBtn) {
    changeLogDirBtn.disabled = true;
  }
  if (alertBtn) {
    alertBtn.disabled = true;
  }
} else {
  startBtn.addEventListener("click", startPing);
  stopBtn.addEventListener("click", stopPing);
  if (changeLogDirBtn) {
    changeLogDirBtn.addEventListener("click", changeLogDir);
  }
  if (alertBtn) {
    alertBtn.addEventListener("click", () => {
      openAlertModal();
      loadAlertSettings();
    });
  }
  if (closeAlertBtn) {
    closeAlertBtn.addEventListener("click", closeAlertModal);
  }
  if (alertModal) {
    alertModal.addEventListener("click", (event) => {
      const target = event.target;
      if (target && target.dataset && target.dataset.close) {
        closeAlertModal();
      }
    });
  }
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeAlertModal();
    }
  });
  document.querySelectorAll(".tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      if (tab.dataset.tab) {
        setActiveTab(tab.dataset.tab);
      }
    });
  });
  if (saveSmtpBtn) {
    saveSmtpBtn.addEventListener("click", saveAlertSettings);
  }
  if (exportAlertBtn) {
    exportAlertBtn.addEventListener("click", exportAlertSettings);
  }
  if (importAlertBtn) {
    importAlertBtn.addEventListener("click", importAlertSettings);
  }
  if (testSmtpBtn) {
    testSmtpBtn.addEventListener("click", testSmtp);
  }
  if (smtpTlsMode && smtpPort) {
    smtpTlsMode.addEventListener("change", () => {
      const current = smtpPort.value.trim();
      const nextPort = smtpTlsMode.value === "starttls" ? "587" : "465";
      const shouldUpdate = !current || current === "465" || current === "587";
      if (shouldUpdate) {
        smtpPort.value = nextPort;
      }
    });
  }
  addressInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && !running) {
      startPing();
    }
  });
  refreshLogDir();
  if (eventApi && typeof eventApi.listen === "function") {
    eventApi.listen("ping-log", (event) => {
      const payload = event && event.payload ? event.payload : null;
      if (!payload) {
        return;
      }
      const { seq, line } = payload;
      addLog({ seq, line });
    }).then((stop) => {
      unlisten = stop;
    }).catch(() => {
      // Ignore listener init errors; logging continues to file.
    });
  }
  if (logList) {
    logList.addEventListener("scroll", () => {
      const distanceToBottom =
        logList.scrollHeight - logList.clientHeight - logList.scrollTop;
      autoScroll = distanceToBottom <= autoScrollThreshold;
      updateScrollButtons();
    });
  }
  if (scrollTopBtn && logList) {
    scrollTopBtn.addEventListener("click", () => {
      logList.scrollTop = 0;
      updateScrollButtons();
    });
  }
  if (scrollBottomBtn && logList) {
    scrollBottomBtn.addEventListener("click", () => {
      logList.scrollTop = logList.scrollHeight - logList.clientHeight;
      updateScrollButtons();
    });
  }
}
