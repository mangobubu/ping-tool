#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::fs::{create_dir_all, read_to_string, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::Local;
use lettre::message::{header::ContentType, Mailbox, Message};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use serde::{Deserialize, Serialize};
use tauri::path::BaseDirectory;
use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

struct PingRunner {
  stop_tx: mpsc::Sender<()>,
  join: thread::JoinHandle<()>,
}

#[derive(Clone, Serialize)]
struct PingEvent {
  seq: u64,
  line: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum TlsMode {
  None,
  Ssl,
  Starttls,
}

impl Default for TlsMode {
  fn default() -> Self {
    TlsMode::Ssl
  }
}

#[derive(Clone, Deserialize, Serialize)]
struct SmtpSettings {
  #[serde(default)]
  host: String,
  #[serde(default = "default_smtp_port")]
  port: u16,
  #[serde(default)]
  username: String,
  #[serde(default)]
  password: String,
  #[serde(default)]
  from: String,
  #[serde(default)]
  to: String,
  #[serde(default)]
  tls_mode: Option<TlsMode>,
  #[serde(default)]
  use_tls: bool,
}

impl Default for SmtpSettings {
  fn default() -> Self {
    Self {
      host: String::new(),
      port: default_smtp_port(),
      username: String::new(),
      password: String::new(),
      from: String::new(),
      to: String::new(),
      tls_mode: Some(TlsMode::Ssl),
      use_tls: false,
    }
  }
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct WechatSettings {
  #[serde(default)]
  enabled: bool,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct AlertSettings {
  #[serde(default)]
  smtp: SmtpSettings,
  #[serde(default)]
  wechat: WechatSettings,
}

#[derive(Default, Deserialize, Serialize)]
struct AppSettings {
  #[serde(default)]
  log_dir: Option<String>,
  #[serde(default)]
  smtp: SmtpSettings,
  #[serde(default)]
  wechat: WechatSettings,
}

#[derive(Clone, Serialize)]
struct LogEntry {
  seq: u64,
  line: String,
}

struct LogBuffer {
  next_seq: u64,
  entries: VecDeque<LogEntry>,
}

struct PingState {
  inner: Mutex<Option<PingRunner>>,
  logs: Arc<Mutex<LogBuffer>>,
}

impl Default for PingState {
  fn default() -> Self {
    Self {
      inner: Mutex::new(None),
      logs: Arc::new(Mutex::new(LogBuffer {
        next_seq: 1,
        entries: VecDeque::with_capacity(100),
      })),
    }
  }
}

#[tauri::command]
fn start_ping(app: AppHandle, state: State<PingState>, address: String) -> Result<String, String> {
  let address = address.trim().to_string();
  if address.is_empty() {
    return Err("Address cannot be empty".to_string());
  }

  let mut guard = state.inner.lock().map_err(|_| "State lock poisoned".to_string())?;
  if guard.is_some() {
    return Err("Ping is already running".to_string());
  }

  let base_dir = resolve_log_base(&app)?;
  let base_dir_clone = base_dir.clone();
  let log_buffer = state.logs.clone();

  if let Ok(mut logs) = log_buffer.lock() {
    logs.entries.clear();
    logs.next_seq = 1;
  }

  let (stop_tx, stop_rx) = mpsc::channel();
  let app_handle = app.clone();
  let join = thread::spawn(move || ping_loop(app_handle, base_dir_clone, address, stop_rx, log_buffer));

  *guard = Some(PingRunner { stop_tx, join });

  Ok(base_dir.to_string_lossy().to_string())
}

#[tauri::command]
fn stop_ping(state: State<PingState>) -> Result<(), String> {
  let mut guard = state.inner.lock().map_err(|_| "State lock poisoned".to_string())?;
  let runner = guard.take().ok_or_else(|| "Ping is not running".to_string())?;

  let _ = runner.stop_tx.send(());
  thread::spawn(move || {
    let _ = runner.join.join();
  });

  Ok(())
}

#[tauri::command]
fn get_recent_logs(state: State<PingState>) -> Result<Vec<LogEntry>, String> {
  let logs = state.logs.lock().map_err(|_| "State lock poisoned".to_string())?;
  Ok(logs.entries.iter().cloned().collect())
}

fn resolve_log_base(app: &AppHandle) -> Result<PathBuf, String> {
  let settings = load_settings(app);
  if let Some(dir) = settings.log_dir {
    return Ok(PathBuf::from(dir));
  }
  app
    .path()
    .resolve("ping-logs", BaseDirectory::AppLog)
    .map_err(|e| e.to_string())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
  app
    .path()
    .resolve("settings.json", BaseDirectory::AppConfig)
    .map_err(|e| e.to_string())
}

fn load_settings(app: &AppHandle) -> AppSettings {
  let path = match settings_path(app) {
    Ok(path) => path,
    Err(_) => return AppSettings::default(),
  };

  match read_to_string(&path) {
    Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
    Err(_) => AppSettings::default(),
  }
}

fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
  let path = settings_path(app)?;
  if let Some(parent) = path.parent() {
    create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  let data = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
  std::fs::write(path, data).map_err(|e| e.to_string())
}

fn default_smtp_port() -> u16 {
  465
}

#[tauri::command]
fn get_log_dir(app: AppHandle) -> Result<String, String> {
  let path = resolve_log_base(&app)?;
  Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn select_log_dir(app: AppHandle) -> Result<String, String> {
  let current = resolve_log_base(&app)?;
  let selected = rfd::FileDialog::new()
    .set_title("选择日志保存目录")
    .set_directory(&current)
    .pick_folder();

  let Some(path) = selected else {
    return Ok(current.to_string_lossy().to_string());
  };

  let mut settings = load_settings(&app);
  settings.log_dir = Some(path.to_string_lossy().to_string());
  save_settings(&app, &settings)?;
  Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_alert_settings(app: AppHandle) -> Result<AlertSettings, String> {
  let settings = load_settings(&app);
  Ok(AlertSettings {
    smtp: settings.smtp,
    wechat: settings.wechat,
  })
}

#[tauri::command]
fn save_alert_settings(app: AppHandle, settings: AlertSettings) -> Result<(), String> {
  let mut existing = load_settings(&app);
  existing.smtp = settings.smtp;
  existing.wechat = settings.wechat;
  save_settings(&app, &existing)
}

#[tauri::command]
fn export_alert_settings(app: AppHandle) -> Result<Option<String>, String> {
  let settings = load_settings(&app);
  let alert = AlertSettings {
    smtp: settings.smtp,
    wechat: settings.wechat,
  };

  let file_path = rfd::FileDialog::new()
    .set_title("导出告警配置")
    .add_filter("JSON", &["json"])
    .set_file_name("alert-settings.json")
    .save_file();

  let Some(path) = file_path else {
    return Ok(None);
  };

  let data = serde_json::to_string_pretty(&alert).map_err(|e| e.to_string())?;
  if let Some(parent) = path.parent() {
    create_dir_all(parent).map_err(|e| e.to_string())?;
  }
  std::fs::write(&path, data).map_err(|e| e.to_string())?;
  Ok(Some(path.to_string_lossy().to_string()))
}

#[tauri::command]
fn import_alert_settings(app: AppHandle) -> Result<Option<AlertSettings>, String> {
  let file_path = rfd::FileDialog::new()
    .set_title("导入告警配置")
    .add_filter("JSON", &["json"])
    .pick_file();

  let Some(path) = file_path else {
    return Ok(None);
  };

  let contents = read_to_string(&path).map_err(|e| e.to_string())?;
  let alert: AlertSettings = serde_json::from_str(&contents).map_err(|e| e.to_string())?;

  let mut existing = load_settings(&app);
  existing.smtp = alert.smtp.clone();
  existing.wechat = alert.wechat.clone();
  save_settings(&app, &existing)?;

  Ok(Some(alert))
}

#[tauri::command]
async fn test_smtp(smtp: SmtpSettings) -> Result<String, String> {
  let mut handle = tauri::async_runtime::spawn_blocking(move || test_smtp_sync(smtp));
  let result = match tokio::time::timeout(Duration::from_secs(15), &mut handle).await {
    Ok(result) => result.map_err(|_| "测试任务被取消".to_string())?,
    Err(_) => {
      // Best-effort abort: blocking task may continue in background.
      handle.abort();
      return Err("连接超时（15 秒）".to_string());
    }
  };
  result
}

fn test_smtp_sync(smtp: SmtpSettings) -> Result<String, String> {
  let host = smtp.host.trim();
  if host.is_empty() {
    return Err("SMTP 主机不能为空".to_string());
  }
  if smtp.port == 0 {
    return Err("SMTP 端口不合法".to_string());
  }
  if smtp.from.trim().is_empty() {
    return Err("发件人邮箱不能为空".to_string());
  }
  if smtp.to.trim().is_empty() {
    return Err("测试收件人邮箱不能为空".to_string());
  }
  let from = smtp
    .from
    .parse::<Mailbox>()
    .map_err(|_| "发件人邮箱格式不正确".to_string())?;
  let to = smtp
    .to
    .parse::<Mailbox>()
    .map_err(|_| "测试收件人邮箱格式不正确".to_string())?;

  let tls_mode = smtp
    .tls_mode
    .clone()
    .unwrap_or_else(|| if smtp.use_tls { TlsMode::Ssl } else { TlsMode::None });

  let base_scheme = match tls_mode {
    TlsMode::Ssl => "smtps",
    _ => "smtp",
  };

  let mut url = Url::parse(&format!("{base_scheme}://localhost")).map_err(|e| e.to_string())?;
  url
    .set_host(Some(host))
    .map_err(|_| "SMTP 主机不合法".to_string())?;
  url
    .set_port(Some(smtp.port))
    .map_err(|_| "SMTP 端口不合法".to_string())?;
  if matches!(tls_mode, TlsMode::Starttls) {
    url
      .query_pairs_mut()
      .append_pair("tls", "required");
  }

  let mut builder = SmtpTransport::from_url(url.as_str())
    .map_err(|e| format!("SMTP 配置无效: {e}\n{:?}", e))?
    .timeout(Some(Duration::from_secs(10)));

  if !smtp.username.is_empty() {
    builder = builder.credentials(Credentials::new(
      smtp.username.clone(),
      smtp.password.clone(),
    ));
  }

  let mailer = builder.build();

  let subject = "Ping Tool 测试邮件";
  let body = format!(
    "这是一封测试邮件，用于验证 SMTP 配置。\n\n发送时间: {}",
    Local::now().format("%Y-%m-%d %H:%M:%S")
  );

  let email = Message::builder()
    .from(from)
    .to(to)
    .subject(subject)
    .header(ContentType::TEXT_PLAIN)
    .body(body)
    .map_err(|e| format!("构建测试邮件失败: {e}"))?;

  mailer
    .send(&email)
    .map_err(|e| format!("发送失败: {e}\n{:?}", e))?;

  Ok("测试邮件已发送。".to_string())
}

fn send_alert_email(smtp: &SmtpSettings, message: &str) -> Result<(), String> {
  let host = smtp.host.trim();
  if host.is_empty() {
    return Err("SMTP 主机未配置".to_string());
  }
  if smtp.port == 0 {
    return Err("SMTP 端口不合法".to_string());
  }
  if smtp.from.trim().is_empty() || smtp.to.trim().is_empty() {
    return Err("SMTP 发件人或收件人未配置".to_string());
  }

  let from = smtp
    .from
    .parse::<Mailbox>()
    .map_err(|_| "发件人邮箱格式不正确".to_string())?;
  let to = smtp
    .to
    .parse::<Mailbox>()
    .map_err(|_| "收件人邮箱格式不正确".to_string())?;

  let tls_mode = smtp
    .tls_mode
    .clone()
    .unwrap_or_else(|| if smtp.use_tls { TlsMode::Ssl } else { TlsMode::None });

  let base_scheme = match tls_mode {
    TlsMode::Ssl => "smtps",
    _ => "smtp",
  };

  let mut url = Url::parse(&format!("{base_scheme}://localhost")).map_err(|e| e.to_string())?;
  url
    .set_host(Some(host))
    .map_err(|_| "SMTP 主机不合法".to_string())?;
  url
    .set_port(Some(smtp.port))
    .map_err(|_| "SMTP 端口不合法".to_string())?;

  if matches!(tls_mode, TlsMode::Starttls) {
    url
      .query_pairs_mut()
      .append_pair("tls", "required");
  }

  let mut builder = SmtpTransport::from_url(url.as_str())
    .map_err(|e| format!("SMTP 配置无效: {e}"))?
    .timeout(Some(Duration::from_secs(10)));

  if !smtp.username.is_empty() {
    builder = builder.credentials(Credentials::new(
      smtp.username.clone(),
      smtp.password.clone(),
    ));
  }

  let mailer = builder.build();
  let subject = "网络丢包告警";
  let email = Message::builder()
    .from(from)
    .to(to)
    .subject(subject)
    .header(ContentType::TEXT_HTML)
    .body(message.to_string())
    .map_err(|e| format!("构建告警邮件失败: {e}"))?;

  mailer
    .send(&email)
    .map_err(|e| format!("发送告警邮件失败: {e}"))?;

  Ok(())
}

fn ping_loop(
  app: AppHandle,
  base_dir: PathBuf,
  address: String,
  stop_rx: mpsc::Receiver<()>,
  log_buffer: Arc<Mutex<LogBuffer>>,
) {
  if let Err(e) = create_dir_all(&base_dir) {
    eprintln!("failed to create log base dir: {e}");
    return;
  }

  let mut fail_count: u32 = 0;
  let mut first_fail_time: Option<String> = None;
  let mut outage_start: Option<String> = None;

  loop {
    if stop_rx.try_recv().is_ok() {
      break;
    }

    let loop_start = Instant::now();
    let now = Local::now();

    let date_folder = now.format("%Y-%m-%d").to_string();
    let hour_folder = now.format("%H").to_string();
    let minute_stamp = now.format("%Y-%m-%d_%H-%M").to_string();
    let timestamp = now.format("%Y-%m-%d %H:%M:%S").to_string();

    let dir = base_dir.join(date_folder).join(hour_folder);
    if let Err(e) = create_dir_all(&dir) {
      eprintln!("failed to create log dir: {e}");
      break;
    }

    let file_path = dir.join(format!("ping_{minute_stamp}.log"));
    let ping_result = ping_once(&address);
    let result = match &ping_result {
      Ok(line) => line.clone(),
      Err(err) => format!("error: {err}"),
    };

    let summary = format!("{address} | {result}");
    let display_line = format!("[{timestamp}] {summary}");
    let file_line = format!("{display_line}\n");
    if let Err(e) = append_line(&file_path, &file_line) {
      eprintln!("failed to write log: {e}");
    }

    let seq = push_log(&log_buffer, display_line.clone());

    let _ = app.emit(
      "ping-log",
      PingEvent {
        seq,
        line: display_line,
      },
    );

    match ping_result {
      Ok(_) => {
        if let Some(start_time) = outage_start.take() {
          let recover_time = timestamp.clone();
          let alert_message_plain = format!(
            "开始时间: {start_time}，恢复时间：{recover_time} 网络出现丢包"
          );
          let alert_message_html = format!(
            "开始时间: {start_time}，<br>恢复时间：{recover_time} <br> 网络出现丢包"
          );
          let alert_line = format!("[{timestamp}] ALERT | {alert_message_plain}\n");
          if let Err(e) = append_line(&file_path, &alert_line) {
            eprintln!("failed to write alert log: {e}");
          } else {
            let _ = push_log(&log_buffer, alert_line.trim_end().to_string());
          }

          let settings = load_settings(&app);
          let smtp = settings.smtp.clone();
          let email_body = alert_message_html.clone();
          thread::spawn(move || {
            if let Err(err) = send_alert_email(&smtp, &email_body) {
              eprintln!("failed to send alert email: {err}");
            }
          });
        }
        fail_count = 0;
        first_fail_time = None;
      }
      Err(_) => {
        fail_count = fail_count.saturating_add(1);
        if fail_count == 1 {
          first_fail_time = Some(timestamp.clone());
        }
        if fail_count == 3 && outage_start.is_none() {
          let start_time = first_fail_time.clone().unwrap_or_else(|| timestamp.clone());
          outage_start = Some(start_time.clone());
          let alert_line = format!("[{timestamp}] ALERT | 连续 3 次失败，开始时间 {start_time}\n");
          if let Err(e) = append_line(&file_path, &alert_line) {
            eprintln!("failed to write alert log: {e}");
          } else {
            let _ = push_log(&log_buffer, alert_line.trim_end().to_string());
          }
        }
      }
    }

    let elapsed = loop_start.elapsed();
    if elapsed < Duration::from_secs(1) {
      let wait = Duration::from_secs(1) - elapsed;
      if stop_rx.recv_timeout(wait).is_ok() {
        break;
      }
    }
  }
}

fn push_log(logs: &Arc<Mutex<LogBuffer>>, entry: String) -> u64 {
  if let Ok(mut logs) = logs.lock() {
    let seq = logs.next_seq;
    logs.next_seq = logs.next_seq.saturating_add(1);
    logs.entries.push_back(LogEntry { seq, line: entry });
    while logs.entries.len() > 100 {
      logs.entries.pop_front();
    }
    return seq;
  }
  0
}

fn ping_once(address: &str) -> Result<String, String> {
  let output = ping_command(address)
    .output()
    .map_err(|e| format!("failed to spawn ping: {e}"))?;

  let stdout = decode_ping_output(&output.stdout);
  let stderr = decode_ping_output(&output.stderr);

  let success = output.status.success();
  let text: &str = if success {
    stdout.as_str()
  } else if !stderr.trim().is_empty() {
    stderr.as_str()
  } else {
    stdout.as_str()
  };

  let lines: Vec<&str> = text
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .collect();

  let preferred = if success {
    select_success_line(&lines)
      .or_else(|| select_non_header_line(&lines))
      .or_else(|| lines.first().copied())
  } else {
    select_error_line(&lines)
      .or_else(|| select_non_header_line(&lines))
      .or_else(|| lines.first().copied())
  };

  let summary = preferred
    .unwrap_or("");

  let summary = if summary.is_empty() {
    format!("ping {address} {}", if success { "ok" } else { "failed" })
  } else {
    summary.to_string()
  };

  if success {
    Ok(summary)
  } else {
    Err(summary)
  }
}

fn select_success_line<'a>(lines: &'a [&'a str]) -> Option<&'a str> {
  lines.iter().copied().find(|line| {
    let lower = line.to_ascii_lowercase();
    line.contains("Reply from")
      || line.contains("bytes from")
      || line.contains("bytes=")
      || lower.contains("time=")
      || lower.contains("time<")
      || lower.contains("ttl=")
      || lower.contains("ms")
      || line.contains("时间")
      || line.contains("字节=")
  })
}

fn select_error_line<'a>(lines: &'a [&'a str]) -> Option<&'a str> {
  lines.iter().copied().find(|line| {
    let lower = line.to_ascii_lowercase();
    lower.contains("timed out")
      || lower.contains("timeout")
      || lower.contains("unreachable")
      || lower.contains("general failure")
      || lower.contains("could not find host")
      || lower.contains("name or service not known")
      || line.contains("请求超时")
      || line.contains("无法访问")
      || line.contains("一般故障")
      || line.contains("找不到主机")
      || line.contains("无法解析")
  })
}

fn select_non_header_line<'a>(lines: &'a [&'a str]) -> Option<&'a str> {
  lines.iter().copied().find(|line| !is_header_line(line))
}

fn is_header_line(line: &str) -> bool {
  let lower = line.to_ascii_lowercase();
  lower.starts_with("pinging ")
    || lower.starts_with("ping ")
    || line.contains("正在 Ping")
    || line.contains("正在ping")
}

#[cfg(target_os = "windows")]
fn ping_command(address: &str) -> Command {
  const CREATE_NO_WINDOW: u32 = 0x08000000;
  let mut cmd = Command::new("ping");
  cmd.args(["-n", "1", address]);
  cmd.creation_flags(CREATE_NO_WINDOW);
  cmd
}

#[cfg(not(target_os = "windows"))]
fn ping_command(address: &str) -> Command {
  let mut cmd = Command::new("ping");
  cmd.args(["-c", "1", address]);
  cmd
}

#[cfg(target_os = "windows")]
fn decode_ping_output(bytes: &[u8]) -> String {
  use windows_sys::Win32::Globalization::GetOEMCP;

  let cp = unsafe { GetOEMCP() };
  match cp {
    65001 => String::from_utf8_lossy(bytes).into_owned(),
    936 => {
      let (cow, _, _) = encoding_rs::GBK.decode(bytes);
      cow.into_owned()
    }
    950 => {
      let (cow, _, _) = encoding_rs::BIG5.decode(bytes);
      cow.into_owned()
    }
    932 => {
      let (cow, _, _) = encoding_rs::SHIFT_JIS.decode(bytes);
      cow.into_owned()
    }
    949 => {
      let (cow, _, _) = encoding_rs::EUC_KR.decode(bytes);
      cow.into_owned()
    }
    _ => String::from_utf8_lossy(bytes).into_owned(),
  }
}

#[cfg(not(target_os = "windows"))]
fn decode_ping_output(bytes: &[u8]) -> String {
  String::from_utf8_lossy(bytes).into_owned()
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
  let mut file = OpenOptions::new().create(true).append(true).open(path)?;
  file.write_all(line.as_bytes())
}

fn main() {
  tauri::Builder::default()
    .manage(PingState::default())
    .invoke_handler(tauri::generate_handler![
      start_ping,
      stop_ping,
      get_recent_logs,
      get_log_dir,
      select_log_dir,
      get_alert_settings,
      save_alert_settings,
      export_alert_settings,
      import_alert_settings,
      test_smtp
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
