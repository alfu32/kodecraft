//! Shared clipboard with a fast local copy epoch and serialized optional native I/O.
//! Native operations are invoked on the platform's blocking workers, never drawing.
use kode_core::{DomainError, Result, services::Clipboard};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

pub trait NativeClipboard: Send {
    fn get_text(&mut self) -> std::result::Result<String, String>;
    fn set_text(&mut self, text: &str) -> std::result::Result<(), String>;
}
struct SystemNative {
    clipboard: Option<arboard::Clipboard>,
}
impl SystemNative {
    fn clipboard(&mut self) -> std::result::Result<&mut arboard::Clipboard, String> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }
        Ok(self.clipboard.as_mut().expect("clipboard initialized"))
    }
}
impl NativeClipboard for SystemNative {
    fn get_text(&mut self) -> std::result::Result<String, String> {
        self.clipboard()?
            .get_text()
            .map_err(|error| error.to_string())
    }
    fn set_text(&mut self, text: &str) -> std::result::Result<(), String> {
        self.clipboard()?
            .set_text(text)
            .map_err(|error| error.to_string())
    }
}
struct InternalOnly;
impl NativeClipboard for InternalOnly {
    fn get_text(&mut self) -> std::result::Result<String, String> {
        Err("internal clipboard".into())
    }
    fn set_text(&mut self, _: &str) -> std::result::Result<(), String> {
        Err("internal clipboard".into())
    }
}

#[derive(Default)]
struct Local {
    text: String,
    epoch: u64,
    synced: u64,
}
pub struct ClipboardService {
    local: Mutex<Local>,
    native: Mutex<Box<dyn NativeClipboard>>,
    available: AtomicBool,
}
impl ClipboardService {
    pub fn new(native: Box<dyn NativeClipboard>) -> Self {
        Self {
            local: Mutex::default(),
            native: Mutex::new(native),
            available: AtomicBool::new(false),
        }
    }
    pub fn internal() -> Self {
        Self::new(Box::new(InternalOnly))
    }
    pub fn system() -> Self {
        Self::new(Box::new(SystemNative { clipboard: None }))
    }
    /// Call immediately for a copy gesture; it never opens a desktop connection.
    pub fn remember(&self, text: &str) -> Result<u64> {
        let mut local = self.local.lock().expect("clipboard mutex");
        let epoch = local
            .epoch
            .checked_add(1)
            .ok_or_else(|| DomainError::Unsupported("clipboard epoch exhausted".into()))?;
        local.text = text.into();
        local.epoch = epoch;
        Ok(epoch)
    }
    pub fn local_text(&self) -> String {
        self.local.lock().expect("clipboard mutex").text.clone()
    }
    pub fn native_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
    /// A late copy cannot overwrite a newer copy. Reads use local pending text
    /// until that epoch is published, making rapid copy/paste deterministic.
    pub fn flush_native(&self, epoch: u64) -> bool {
        let mut native = self.native.lock().expect("native clipboard mutex");
        let value = {
            let local = self.local.lock().expect("clipboard mutex");
            if local.epoch != epoch {
                return false;
            }
            local.text.clone()
        };
        let success = native.set_text(&value).is_ok();
        self.available.store(success, Ordering::Release);
        if success {
            let mut local = self.local.lock().expect("clipboard mutex");
            if local.epoch == epoch {
                local.synced = epoch;
            }
        }
        success
    }
}
impl Clipboard for ClipboardService {
    fn read(&self) -> Result<String> {
        let epoch = {
            let local = self.local.lock().expect("clipboard mutex");
            if local.epoch > local.synced {
                return Ok(local.text.clone());
            }
            local.epoch
        };
        let native = self
            .native
            .lock()
            .expect("native clipboard mutex")
            .get_text();
        let mut local = self.local.lock().expect("clipboard mutex");
        if local.epoch != epoch {
            return Ok(local.text.clone());
        }
        if let Ok(text) = native {
            local.epoch = local
                .epoch
                .checked_add(1)
                .ok_or_else(|| DomainError::Unsupported("clipboard epoch exhausted".into()))?;
            local.synced = local.epoch;
            local.text = text;
            self.available.store(true, Ordering::Release);
        } else {
            self.available.store(false, Ordering::Release);
        }
        Ok(local.text.clone())
    }
    fn write(&self, text: &str) -> Result<()> {
        let epoch = self.remember(text)?;
        self.flush_native(epoch);
        Ok(())
    }
}
