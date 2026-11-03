use serde_json::Value;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

struct Process {
    child: Child,
    input: ChildStdin,
    chunks: Receiver<std::io::Result<Vec<u8>>>,
    pending: Vec<u8>,
    next_execute: u64,
}

impl Process {
    fn start(executable: &Path) -> std::io::Result<Self> {
        let mut command = Command::new(executable);
        command
            .args(["-stay_open", "True", "-@", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
        let mut child = command.spawn()?;
        let mut output = child.stdout.take().unwrap();
        let (send, chunks) = mpsc::channel();
        thread::Builder::new()
            .name("exiftool-stdout".into())
            .spawn(move || {
                let mut buf = [0u8; 64 * 1024];
                loop {
                    match output.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if send.send(Ok(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = send.send(Err(e));
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            input: child.stdin.take().unwrap(),
            child,
            chunks,
            pending: Vec::new(),
            next_execute: 1,
        })
    }

    fn request(&mut self, args: &[String]) -> std::io::Result<Vec<u8>> {
        if self.child.try_wait()?.is_some() {
            return Err(io_error(std::io::ErrorKind::BrokenPipe, "ExifTool exited"));
        }
        let execute = self.next_execute;
        self.next_execute += 1;
        let marker = format!("{{ready{execute}}}").into_bytes();
        for arg in args {
            writeln!(self.input, "{arg}")?;
        }
        writeln!(self.input, "-execute{execute}")?;
        self.input.flush()?;
        let deadline = std::time::Instant::now() + REQUEST_TIMEOUT;
        loop {
            if let Some(frame) = take_frame(&mut self.pending, &marker) {
                return Ok(frame);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(io_error(std::io::ErrorKind::TimedOut, "ExifTool request timed out"));
            }
            match self.chunks.recv_timeout(remaining) {
                Ok(Ok(chunk)) => self.pending.extend_from_slice(&chunk),
                Ok(Err(e)) => return Err(e),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(io_error(std::io::ErrorKind::TimedOut, "ExifTool request timed out"));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io_error(std::io::ErrorKind::UnexpectedEof, "ExifTool closed stdout"));
                }
            }
        }
    }
}

fn take_frame(buffer: &mut Vec<u8>, marker: &[u8]) -> Option<Vec<u8>> {
    // ExifTool's marker is terminated by LF (or CRLF). Restrict recognition to
    // the end of the accumulated stream so binary payloads containing marker-like
    // bytes cannot prematurely complete a request.
    let (end, terminator_len) = if buffer.ends_with(b"\r\n") {
        (buffer.len() - 2, 2)
    } else if buffer.ends_with(b"\n") {
        (buffer.len() - 1, 1)
    } else if buffer.ends_with(marker) {
        (buffer.len(), 0)
    } else {
        return None;
    };
    if end < marker.len() || &buffer[end - marker.len()..end] != marker {
        return None;
    }
    let payload_end = end - marker.len();
    let payload = buffer[..payload_end].to_vec();
    buffer.drain(..end + terminator_len);
    Some(payload)
}

fn io_error(kind: std::io::ErrorKind, message: &str) -> std::io::Error {
    std::io::Error::new(kind, message.to_owned())
}

fn kill_child_bounded(child: &mut Child) {
    // Never wait for ExifTool's protocol on shutdown: it may be stuck in a
    // request. Kill directly, then reap only for a short bounded interval.
    let _ = child.kill();
    let deadline = std::time::Instant::now() + Duration::from_millis(300);
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) { kill_child_bounded(&mut self.child); }
}

pub struct ExifTool {
    executable: PathBuf,
    pool: [Mutex<Option<Process>>; 2],
    next: AtomicUsize,
    shutting_down: AtomicBool,
}

impl ExifTool {
    pub fn new(executable: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            executable,
            pool: [Mutex::new(None), Mutex::new(None)],
            next: AtomicUsize::new(0),
            shutting_down: AtomicBool::new(false),
        })
    }

    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for slot in &self.pool {
            if let Ok(mut process) = slot.lock() { *process = None; }
        }
    }

    fn with_process<T>(&self, f: impl FnOnce(&mut Process) -> std::io::Result<T>) -> std::io::Result<T> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(io_error(std::io::ErrorKind::BrokenPipe, "ExifTool pool is shut down"));
        }
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.pool.len();
        let mut slot = self.pool[idx].lock().map_err(|_| io_error(std::io::ErrorKind::Other, "ExifTool pool poisoned"))?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(io_error(std::io::ErrorKind::BrokenPipe, "ExifTool pool is shut down"));
        }
        let dead = match slot.as_mut() { None => true, Some(p) => p.child.try_wait()?.is_some() };
        if dead { *slot = Some(Process::start(&self.executable)?); }
        let result = f(slot.as_mut().unwrap());
        if result.is_err() {
            // In particular, timeouts invalidate the process. Dropping it kills
            // ExifTool and closes its reader channel before releasing this mutex.
            *slot = None;
        }
        result
    }

    pub fn metadata(&self, path: &Path) -> std::io::Result<Value> {
        let p = path.to_string_lossy().to_string();
        let data = self.with_process(|proc| proc.request(&[
            "-charset".into(), "filename=UTF8".into(), "-j".into(), "-G1".into(), "-s".into(), p,
        ]))?;
        let json: Value = serde_json::from_slice(&data).map_err(|e| io_error(std::io::ErrorKind::InvalidData, &e.to_string()))?;
        Ok(json.as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null))
    }

    pub fn metadata_batch(&self, paths: &[PathBuf], tags: &[&str]) -> std::io::Result<Vec<Value>> {
        if paths.is_empty() { return Ok(Vec::new()); }
        let mut args=vec!["-charset".into(),"filename=UTF8".into(),"-fast2".into(),"-j".into(),"-G1".into(),"-s".into()];
        for tag in tags { args.push(format!("-{tag}")); }
        args.extend(paths.iter().map(|p|p.to_string_lossy().to_string()));
        let data=self.with_process(|proc|proc.request(&args))?;
        let json:Value=serde_json::from_slice(&data).map_err(|e|io_error(std::io::ErrorKind::InvalidData,&e.to_string()))?;
        Ok(json.as_array().cloned().unwrap_or_default())
    }

    pub fn largest_preview(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.largest_preview_with_orientation(path).map(|(preview,_)|preview)
    }

    pub fn largest_preview_with_orientation(&self, path: &Path) -> std::io::Result<(Vec<u8>,Option<u8>)> {
        let p = path.to_string_lossy().to_string();
        let data = self.with_process(|proc| proc.request(&[
            "-charset".into(), "filename=UTF8".into(), "-a".into(), "-G1".into(), "-j".into(),
            "-PreviewImage".into(), "-JpgFromRaw".into(), "-OtherImage".into(),
            "-ThumbnailImage".into(), "-ThumbnailTIFF".into(), "-PhotoshopThumbnail".into(), "-n".into(), "-Orientation".into(), p.clone(),
        ]))?;
        let json: Value = serde_json::from_slice(&data).map_err(|e| io_error(std::io::ErrorKind::InvalidData, &e.to_string()))?;
        let object = json.as_array().and_then(|a| a.first()).and_then(Value::as_object)
            .ok_or_else(|| io_error(std::io::ErrorKind::NotFound, "No embedded preview found"))?;
        let orientation=orientation_value(object);
        let selected = largest_preview_key(object)
            .ok_or_else(|| io_error(std::io::ErrorKind::NotFound, "No embedded preview found"))?;
        let selector = selected.rsplit_once(':').map(|(group,tag)|format!("-{group}:{tag}")).unwrap_or_else(||format!("-{selected}"));
        let preview=self.with_process(|proc| proc.request(&[
            "-charset".into(), "filename=UTF8".into(), "-b".into(), selector, p,
        ]))?;
        Ok((preview,orientation))
    }

}

fn orientation_value(object:&serde_json::Map<String,Value>)->Option<u8>{
    object.iter().find(|(key,_)|key.ends_with(":Orientation")&&key.starts_with("IFD0:"))
        .or_else(||object.iter().find(|(key,_)|key.ends_with(":Orientation")))
        .and_then(|(_,v)|v.as_u64().or_else(||v.as_str()?.parse().ok())).and_then(|v|u8::try_from(v).ok())
}

fn largest_preview_key(object: &serde_json::Map<String, Value>) -> Option<String> {
    object.iter().filter_map(|(key,value)| {
        let length = value.as_str()?.strip_prefix("(Binary data ")?.split_once(" bytes")?.0.parse::<usize>().ok()?;
        Some((length, key.clone()))
    }).max_by_key(|(length,_)|*length).map(|(_,key)|key)
}

#[cfg(test)]
mod tests {
    use super::{kill_child_bounded, largest_preview_key, take_frame, ExifTool};
    use serde_json::json;

    #[test]
    fn binary_frame_with_fake_ready_prefix_and_split_marker() {
        let marker = b"{ready42}";
        let payload = b"\xff\xd8binary{ready but not the marker}\x00\xff\xd9";
        let mut buffer = payload.to_vec();
        buffer.extend_from_slice(b"{ready");
        assert!(take_frame(&mut buffer, marker).is_none());
        buffer.extend_from_slice(b"42}\r\n");
        assert_eq!(take_frame(&mut buffer, marker).unwrap(), payload);
        assert!(buffer.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn bounded_shutdown_kills_a_stuck_child() {
        use std::process::{Command, Stdio};
        let mut child=Command::new("ping.exe").args(["-n","30","127.0.0.1"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let started=std::time::Instant::now();
        kill_child_bounded(&mut child);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn shutdown_is_idempotent_and_prevents_restart() {
        let tool = ExifTool::new(std::path::PathBuf::from("missing-exiftool"));
        tool.shutdown();
        tool.shutdown();
        assert!(tool.metadata(std::path::Path::new("ignored.jpg")).is_err());
    }

    #[test]
    fn preview_selection_uses_largest_grouped_duplicate() {
        let value=json!({"SubIFD1:PreviewImage":"(Binary data 2600000 bytes, use -b option to extract)","Pentax:PreviewImage":"(Binary data 54000 bytes, use -b option to extract)","IFD0:ThumbnailImage":"(Binary data 12000 bytes, use -b option to extract)"});
        assert_eq!(largest_preview_key(value.as_object().unwrap()).as_deref(),Some("SubIFD1:PreviewImage"));
    }
}

impl Drop for ExifTool {
    fn drop(&mut self) { self.shutdown(); }
}
