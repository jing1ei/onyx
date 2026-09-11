//! Native file boundary for the independent audio editing window.
use std::{fs, path::{Path, PathBuf}, process::Command, sync::{Mutex, Arc}};
use serde::{Serialize, Deserialize};
use tauri::{Manager, AppHandle, Emitter};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use base64::Engine;
const MAX_PCM_BYTES: usize = 256 * 1024 * 1024;
const MAX_WAV_BYTES: usize = MAX_PCM_BYTES + 64 * 1024;
const MAX_ENCODED_BYTES: usize = 4 * ((MAX_WAV_BYTES + 2) / 3);
static ENGLISH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn label<'a>(en: &'a str, zh: &'a str) -> &'a str { if ENGLISH.load(std::sync::atomic::Ordering::Relaxed) { en } else { zh } }

#[derive(Default)]
pub struct EditorState(pub Mutex<Option<Document>>, crate::editor_cache::LatestCache<(PathBuf, Arc<Vec<u8>>), Prepared>);
struct Prepared { path: PathBuf, original: Arc<Vec<u8>>, codec: String, data: Arc<Vec<u8>>, id: String }
pub struct Document { path: PathBuf, original: Arc<Vec<u8>>, codec: String, data: Arc<Vec<u8>> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply { canceled: bool, name: Option<String>, data: Option<String>, path: Option<String>, byte_length: Option<usize>, prepared_id: Option<String> }
impl Reply {
    fn canceled() -> Self { Self { canceled: true, name: None, data: None, path: None, byte_length: None, prepared_id: None } }
    fn file(path: &Path, data: Option<Vec<u8>>) -> Self {
        Self { canceled: false, prepared_id: None, byte_length: data.as_ref().map(|d| d.len()), path: Some(path.to_string_lossy().into_owned()), name: path.file_name().map(|s| s.to_string_lossy().into_owned()), data: data.map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)) }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request { pub action: String, pub path: Option<String>, pub bytes: Option<String>, pub suggested_name: Option<String>, pub language: Option<String>, pub offset: Option<usize>, pub prepared_id: Option<String> }

fn command(tool: &str) -> Command {
    let local = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.join(format!("{tool}.exe"))));
    let path = local.filter(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(tool));
    let mut cmd = Command::new(path);
    #[cfg(windows)] { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }
    cmd
}
fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| format!("无法启动音频转换工具：{e}"))?;
    if !out.status.success() { return Err(String::from_utf8_lossy(&out.stderr).chars().take(1000).collect()); }
    Ok(out.stdout)
}
fn io(e: std::io::Error) -> String { e.to_string() }
fn temp_path(target: &Path, suffix: &str) -> PathBuf {
    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    target.with_file_name(format!(".onyx-{}-{id}.{suffix}", std::process::id()))
}
struct Temp(PathBuf);
pub struct Preview { pub path: PathBuf }
impl Drop for Preview { fn drop(&mut self) { let _ = fs::remove_file(&self.path); } }

// The preview is a temporary PCM file, never the save target. The loader keeps
// the original identity and routes all native playback/EQ through this version.
fn sync_player(app: &AppHandle, path: &Path, bytes: Option<Vec<u8>>) -> Result<(), String> {
    use onyx_core::Deck;
    let state = app.state::<std::sync::Arc<crate::state::AppState>>();
    state.blind_guard("Editing audio")?;
    state.engine.pause();
    let canonical = fs::canonicalize(path).map_err(io)?;
    if let Some(bytes) = bytes {
        let preview = std::sync::Arc::new(Preview { path: temp_path(&std::env::temp_dir().join("onyx"), "wav") });
        fs::write(&preview.path, bytes).map_err(io)?;
        // Validate before making the preview authoritative.
        crate::safe_decode::probe_with(&preview.path, &state.decode_options())?;
        state.editor_previews.lock().insert(canonical.clone(), preview);
    } else {
        state.editor_previews.lock().remove(&canonical);
    }
    let mut ids: Vec<u64> = state.snapshot().playlist.iter().filter(|e| fs::canonicalize(&e.path).ok().as_ref() == Some(&canonical)).map(|e| e.id).collect();
    if ids.is_empty() {
        let id = state.playlist.lock().add_source(path, None);
        ids.push(id);
    }
    let assigned: Vec<(Deck, u64)> = {
        let decks = state.decks.lock();
        [Deck::A, Deck::B].into_iter().filter_map(|d| decks[d.index()].entry_id.filter(|id| ids.contains(id)).map(|id| (d,id))).collect()
    };
    for id in &ids { if let Some(e) = state.playlist.lock().get_mut(*id) { e.probed = false; e.analysis = None; } }
    if assigned.is_empty() {
        crate::loader::load_entry_into_deck(state.inner(), state.engine.active_deck(), ids[0], false)?;
    } else {
        for (deck,id) in assigned { crate::loader::load_entry_into_deck(state.inner(), deck, id, false)?; }
    }
    state.engine.seek_secs(0.0);
    state.emit_state();
    Ok(())
}
impl Drop for Temp { fn drop(&mut self) { let _ = fs::remove_file(&self.0); } }

fn prepare(app: &AppHandle, path: PathBuf) -> Result<Arc<Prepared>, String> {
        let path = fs::canonicalize(path).map_err(io)?;
        if fs::metadata(&path).map_err(io)?.len() > 256 * 1024 * 1024 { return Err("当前编辑模式支持 256 MB 以内的源文件".into()); }
        let state = app.state::<EditorState>();
        let original = Arc::new(fs::read(&path).map_err(io)?);
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        state.1.prepare((path.clone(), original.clone()), |cancelled| {
        // PCM WAV already contains editable samples. Keep its original bytes
        // and codec instead of probing, converting and writing a second WAV.
        if let Some(info) = crate::editor_wav::inspect(&original) {
            if info.decoded_bytes > MAX_PCM_BYTES { return Err("解码音频超过 256 MiB 编辑上限，请先缩短音频".into()); }
            if fs::read(&path).map_err(io)? != *original { return Err("文件在读取过程中被修改，请重新打开".into()); }
            return Ok(Prepared { path, data: original.clone(), original, codec: info.codec.into(), id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string() });
        }
        if cancelled.load(std::sync::atomic::Ordering::Acquire) { return Err("编辑预载已过期".into()); }
        let info = run(command("ffprobe").args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name,sample_rate,channels,duration:format=duration", "-of", "json"]).arg(&path))?;
        let probe: serde_json::Value = serde_json::from_slice(&info).map_err(|e| e.to_string())?;
        if cancelled.load(std::sync::atomic::Ordering::Acquire) { return Err("编辑预载已过期".into()); }
        let stream = &probe["streams"][0];
        let rate = stream["sample_rate"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
        let channels = stream["channels"].as_u64().unwrap_or(0);
        if !(1..=2).contains(&channels) { return Err("编辑模式目前支持单声道或双声道音频".into()); }
        if !rate.is_finite() || rate <= 0.0 { return Err("无法读取有效的音频采样率".into()); }
        let duration = [&stream["duration"], &probe["format"]["duration"]].into_iter()
            .filter_map(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64()))
            .find(|d| d.is_finite() && *d > 0.0);
        let max_seconds = MAX_PCM_BYTES as f64 / (rate * channels as f64 * 4.0);
        if duration.is_some_and(|d| d > max_seconds) {
            return Err("解码音频超过 256 MiB 编辑上限，请先缩短音频".into());
        }
        let wav = Temp(temp_path(&std::env::temp_dir().join("onyx"), "wav"));
        // Unknown/inaccurate duration metadata is allowed. Decode at most one
        // second beyond the budget, then reject oversized output (never edit
        // a silently truncated file). Check disk size before allocating it.
        run(command("ffmpeg").args(["-v", "error", "-nostdin", "-y", "-i"]).arg(&path)
            .args(["-map", "0:a:0", "-c:a", "pcm_f32le", "-t", &(MAX_WAV_BYTES as f64 / (rate * channels as f64 * 4.0) + 1.0).to_string()]).arg(&wav.0))?;
        if fs::metadata(&wav.0).map_err(io)?.len() > MAX_WAV_BYTES as u64 {
            return Err("解码音频超过 256 MiB 编辑上限，请先缩短音频".into());
        }
        let data = fs::read(&wav.0).map_err(io)?;
        if fs::read(&path).map_err(io)? != *original { return Err("文件在读取过程中被修改，请重新打开".into()); }
        Ok(Prepared { path, original, codec: stream["codec_name"].as_str().unwrap_or("").into(), data: Arc::new(data), id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string() })
        })

}

pub fn read_chunk(app: &AppHandle, req: Request) -> Result<Vec<u8>, String> {
    let state = app.state::<EditorState>();
    if let Some(id) = req.prepared_id {
        let cache = state.1.ready()?;
        let hit = cache.as_ref().filter(|c| c.id == id && req.path.as_deref() == Some(c.path.to_string_lossy().as_ref())).ok_or("编辑缓存已变化，请重试")?;
        let start = req.offset.unwrap_or(0);
        if start >= hit.data.len() { return Err("无效音频数据偏移".into()); }
        return Ok(hit.data[start..start.saturating_add(8 * 1024 * 1024).min(hit.data.len())].to_vec());
    }
    let document = state.0.lock().map_err(|_| "编辑文档不可用")?;
    let doc = document.as_ref().ok_or("请先打开音频")?;
    if req.path.as_deref() != Some(doc.path.to_string_lossy().as_ref()) { return Err("编辑文档已变化，请重试".into()); }
    let start = req.offset.unwrap_or(0);
    if start >= doc.data.len() { return Err("无效音频数据偏移".into()); }
    let end = start.saturating_add(8 * 1024 * 1024).min(doc.data.len());
    Ok(doc.data[start..end].to_vec())
}

pub fn handle(app: &AppHandle, req: Request) -> Result<Reply, String> {
    if let Some(language) = &req.language { ENGLISH.store(language == "en", std::sync::atomic::Ordering::Relaxed); }
    if req.action == "language" { return Ok(Reply::canceled()); }
    if req.action == "enter" || req.action == "leave" {
        let player = app.state::<std::sync::Arc<crate::state::AppState>>();
        if req.action == "enter" { player.blind_guard("Editing audio")?; }
        player.engine.set_playback_blocked(req.action == "enter");
        return Ok(Reply::canceled());
    }
    if req.action == "window" {
        app.emit("onyx://editor-open", ()).map_err(|e| e.to_string())?;
        return Ok(Reply::canceled());
    }
    if req.action == "prepare" {
        let path = req.path.ok_or("缺少音频路径")?;
        let ready = prepare(app, PathBuf::from(path))?;
        let mut reply = Reply::file(&ready.path, None);
        reply.byte_length = Some(ready.data.len()); reply.prepared_id = Some(ready.id.clone());
        return Ok(reply);
    }
    let state = app.state::<EditorState>();
    let mut document = state.0.lock().map_err(|_| "编辑文档不可用")?;
    if req.action == "open" {
        let path = if let Some(path) = req.path { PathBuf::from(path) } else {
            match app.dialog().file().set_title(label("Open audio", "打开音频")).add_filter(label("Audio", "音频"), &["wav", "mp3", "flac", "m4a", "aac", "ogg", "aiff", "opus"]).blocking_pick_file() {
                Some(p) => p.into_path().map_err(|e| e.to_string())?, None => return Ok(Reply::canceled())
            }
        };
        let prepared = prepare(app, path)?;
        let path = prepared.path.clone();
        let original = prepared.original.clone();
        let mut reply = Reply::file(&path, None);
        reply.byte_length = Some(prepared.data.len());
        reply.prepared_id = Some(prepared.id.clone());
        if let Some(old) = document.as_ref().filter(|old| old.path != path) {
            let player = app.state::<std::sync::Arc<crate::state::AppState>>();
            let had_preview = player.editor_previews.lock().remove(&old.path).is_some();
            let snap = player.snapshot();
            let loaded = [&snap.deck_a, &snap.deck_b].into_iter().any(|d| d.state.info.as_ref().and_then(|i| fs::canonicalize(&i.path).ok()).as_ref() == Some(&old.path));
            if had_preview && loaded { sync_player(app, &old.path, None)?; }
        }
        *document = Some(Document { path, original, codec: prepared.codec.clone(), data: prepared.data.clone() });
        return Ok(reply);
    }
    if req.action == "attach" || req.action == "restore" {
        let doc = document.as_ref().ok_or("请先打开音频")?;
        sync_player(app, &doc.path, None)?;
        if req.action == "attach" {
            // Opening a playlist row in Edit makes that document the audible
            // source too, even if it was previously assigned only to deck B.
            let player = app.state::<std::sync::Arc<crate::state::AppState>>();
            let deck = player.engine.active_deck();
            let snap = player.snapshot();
            let slot = if deck == onyx_core::Deck::A { &snap.deck_a } else { &snap.deck_b };
            let current = slot.state.info.as_ref().and_then(|i| fs::canonicalize(&i.path).ok());
            if current.as_ref() != Some(&doc.path) {
                let id = snap.playlist.iter()
                    .find(|e| fs::canonicalize(&e.path).ok().as_ref() == Some(&doc.path))
                    .map(|e| e.id).ok_or("无法同步当前剪辑音频")?;
                crate::loader::load_entry_into_deck(player.inner(), deck, id, false)?;
            }
        }
        return Ok(Reply::file(&doc.path, None));
    }
    if req.action != "overwrite" && req.action != "saveAs" && req.action != "preview" { return Err("未知编辑操作".into()); }
    let doc = document.as_mut().ok_or("请先打开音频")?;
    let encoded = req.bytes.ok_or("缺少保存内容")?;
    if encoded.len() > MAX_ENCODED_BYTES { return Err("保存内容超出内存上限".into()); }
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || bytes.len() > MAX_WAV_BYTES || &bytes[..4] != b"RIFF" { return Err("保存的音频数据无效".into()); }
    if req.action == "preview" {
        sync_player(app, &doc.path, Some(bytes))?;
        return Ok(Reply::file(&doc.path, None));
    }
    let target = if req.action == "overwrite" { doc.path.clone() } else {
        match app.dialog().file().set_file_name(req.suggested_name.unwrap_or_else(|| "edited.wav".into()))
            .set_title(label("Save audio as", "音频另存为")).add_filter(label("Audio", "音频"), &["wav", "mp3", "flac", "m4a", "aac", "ogg", "aiff", "opus"]).blocking_save_file() {
            Some(p) => p.into_path().map_err(|e| e.to_string())?, None => return Ok(Reply::canceled())
        }
    };
    let same = fs::canonicalize(&target).ok().as_ref() == Some(&doc.path);
    if same && fs::read(&doc.path).map_err(io)? != *doc.original { return Err("原文件已被其他程序修改，请另存为，避免覆盖外部修改".into()); }
    let ext = target.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let codec = match ext.as_str() {
        "wav" => if same && doc.codec.starts_with("pcm_") { doc.codec.as_str() } else { "pcm_s24le" },
        "mp3" => "libmp3lame", "flac" => "flac", "m4a" | "aac" => "aac", "ogg" => "libvorbis", "opus" => "libopus", "aiff" | "aif" => "pcm_s24be",
        _ => return Err("请选择 WAV、MP3、FLAC、M4A、AAC、OGG、Opus 或 AIFF 格式".into())
    };
    let input = Temp(temp_path(&target, "wav"));
    let output = Temp(temp_path(&target, &ext));
    fs::write(&input.0, bytes).map_err(io)?;
    let mut encode = command("ffmpeg");
    encode.args(["-v", "error", "-nostdin", "-y", "-i"]).arg(&input.0).args(["-c:a", codec]);
    if ext == "mp3" { encode.args(["-b:a", "320k"]); }
    if ext == "m4a" || ext == "aac" { encode.args(["-b:a", "256k"]); }
    run(encode.arg(&output.0))?;
    run(command("ffmpeg").args(["-v", "error", "-xerror", "-nostdin", "-i"]).arg(&output.0).args(["-f", "null", "-"]))?;
    fs::OpenOptions::new().write(true).open(&output.0).map_err(io)?.sync_all().map_err(io)?;
    let backup = temp_path(&target, "backup");
    let existed = target.exists();
    if existed && fs::metadata(&target).map_err(io)?.permissions().readonly() { return Err("目标文件为只读，未覆盖原文件".into()); }
    if same && fs::read(&doc.path).map_err(io)? != *doc.original { return Err("原文件在编码过程中被修改，未覆盖原文件".into()); }
    if existed { fs::rename(&target, &backup).map_err(io)?; }
    if let Err(e) = fs::rename(&output.0, &target) {
        if existed { if let Err(restore) = fs::rename(&backup, &target) { return Err(format!("保存失败：{e}；原文件保留在 {}（恢复失败：{restore}）", backup.display())); } }
        return Err(e.to_string());
    }
    if existed { let _ = fs::remove_file(&backup); }
    let old_path = doc.path.clone();
    doc.path = fs::canonicalize(&target).map_err(io)?;
    doc.original = Arc::new(fs::read(&doc.path).map_err(io)?);
    doc.codec = codec.to_owned();
    if old_path != doc.path {
        // Save-as starts a new file identity; the old row must return to its
        // unchanged original rather than retain an orphaned edit preview.
        let player = app.state::<std::sync::Arc<crate::state::AppState>>();
        player.editor_previews.lock().remove(&old_path);
        if let Err(error) = sync_player(app, &old_path, None) { log::warn!("Could not refresh previous source: {error}"); }
    }
    let english = req.language.as_deref() == Some("en");
    if let Err(e) = app.notification().builder().title(if english { "Onyx · Saved successfully" } else { "Onyx · 保存成功" })
        .body(format!("{} {}", if english { "Saved" } else { "已保存" }, target.file_name().unwrap_or_default().to_string_lossy())).show() {
        log::warn!("Save notification unavailable: {e}");
    }
    Ok(Reply::file(&target, None))
}
