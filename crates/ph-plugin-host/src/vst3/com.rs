//! Host-side COM objects the VST3 host hands to plugins: the host context
//! (`IHostApplication`, which also creates `IMessage`/`IAttributeList`), memory streams
//! (`IBStream`), the component handler (`IComponentHandler`, the controller's edits), the plug
//! frame (`IPlugFrame`), and the real-time parameter-change and event lists.
//!
//! The parameter-change and event lists are allocated once in `activate` with fixed capacity
//! and only touched by the audio thread (and the plugin during `process`), so filling and
//! reading them allocates nothing.

#![allow(non_snake_case)]

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::*};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper, Interface};

fn iid_eq(a: *const TUID, b: &[u8; 16]) -> bool {
    // SAFETY: callers pass valid 16-byte ids
    unsafe { (*a).iter().zip(b.iter()).all(|(x, y)| *x as u8 == *y) }
}

/// Writes `s` as UTF-16 into a VST3 `String128`.
pub fn write_string128(s: &str, dst: &mut String128) {
    let mut n = 0;
    for (d, c) in dst.iter_mut().zip(s.encode_utf16()) {
        *d = c as TChar;
        n += 1;
    }
    let n = n.min(dst.len() - 1);
    dst[n] = 0;
}

/// Reads a NUL-terminated UTF-16 buffer.
pub fn read_tchars(s: &[TChar]) -> String {
    let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..end].iter().map(|c| *c as u16).collect::<Vec<_>>())
        .trim()
        .to_string()
}

// ------------------------------------------------------------------------------- host context

pub struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        // SAFETY: the plugin passes a String128
        write_string128("Patternhaus", unsafe { &mut *name });
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        _iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if iid_eq(cid, &IMessage::IID) {
            let m = ComWrapper::new(Message {
                id: Mutex::new(None),
                attrs: ComWrapper::new(AttrList::default()),
            });
            // SAFETY: handing out one counted reference
            unsafe { *obj = m.to_com_ptr::<IMessage>().unwrap().into_raw() as *mut c_void };
            return kResultOk;
        }
        if iid_eq(cid, &IAttributeList::IID) {
            let a = ComWrapper::new(AttrList::default());
            // SAFETY: as above
            unsafe { *obj = a.to_com_ptr::<IAttributeList>().unwrap().into_raw() as *mut c_void };
            return kResultOk;
        }
        // SAFETY: COM convention: clear the out pointer on failure
        unsafe { *obj = std::ptr::null_mut() };
        kNoInterface
    }
}

pub struct Message {
    id: Mutex<Option<std::ffi::CString>>,
    attrs: ComWrapper<AttrList>,
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id
            .lock()
            .unwrap()
            .as_ref()
            .map_or(std::ptr::null(), |s| s.as_ptr())
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        let v = if id.is_null() {
            None
        } else {
            // SAFETY: a NUL-terminated id from the plugin
            Some(unsafe { CStr::from_ptr(id) }.to_owned())
        };
        *self.id.lock().unwrap() = v;
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        self.attrs.as_com_ref::<IAttributeList>().unwrap().as_ptr()
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

#[derive(Default)]
pub struct AttrList {
    map: Mutex<HashMap<Vec<u8>, Attr>>,
}

impl Class for AttrList {
    type Interfaces = (IAttributeList,);
}

fn key(id: IAttributeList_::AttrID) -> Vec<u8> {
    if id.is_null() {
        return Vec::new();
    }
    // SAFETY: attribute ids are NUL-terminated strings
    unsafe { CStr::from_ptr(id) }.to_bytes().to_vec()
}

impl IAttributeListTrait for AttrList {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: int64) -> tresult {
        self.map.lock().unwrap().insert(key(id), Attr::Int(value));
        kResultOk
    }
    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut int64) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Int(v)) => {
                // SAFETY: out pointer from the plugin
                unsafe { *value = *v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        self.map.lock().unwrap().insert(key(id), Attr::Float(value));
        kResultOk
    }
    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Float(v)) => {
                // SAFETY: out pointer from the plugin
                unsafe { *value = *v };
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        let mut v = Vec::new();
        if !string.is_null() {
            let mut i = 0;
            // SAFETY: a NUL-terminated UTF-16 string
            while unsafe { *string.add(i) } != 0 {
                v.push(unsafe { *string.add(i) });
                i += 1;
            }
        }
        self.map.lock().unwrap().insert(key(id), Attr::Str(v));
        kResultOk
    }
    unsafe fn getString(
        &self,
        id: IAttributeList_::AttrID,
        string: *mut TChar,
        sizeInBytes: uint32,
    ) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Str(v)) => {
                let cap = sizeInBytes as usize / std::mem::size_of::<TChar>();
                if cap == 0 {
                    return kResultFalse;
                }
                let n = v.len().min(cap - 1);
                // SAFETY: the plugin's buffer holds `cap` characters
                unsafe {
                    std::ptr::copy_nonoverlapping(v.as_ptr(), string, n);
                    *string.add(n) = 0;
                }
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn setBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *const c_void,
        sizeInBytes: uint32,
    ) -> tresult {
        let v = if data.is_null() {
            Vec::new()
        } else {
            // SAFETY: the plugin's buffer of that size
            unsafe { std::slice::from_raw_parts(data as *const u8, sizeInBytes as usize) }.to_vec()
        };
        self.map.lock().unwrap().insert(key(id), Attr::Bin(v));
        kResultOk
    }
    unsafe fn getBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *mut *const c_void,
        sizeInBytes: *mut uint32,
    ) -> tresult {
        match self.map.lock().unwrap().get(&key(id)) {
            Some(Attr::Bin(v)) => {
                // SAFETY: the bytes live as long as the list (the plugin's contract)
                unsafe {
                    *data = v.as_ptr() as *const c_void;
                    *sizeInBytes = v.len() as uint32;
                }
                kResultOk
            }
            _ => kResultFalse,
        }
    }
}

// ------------------------------------------------------------------------------- streams

/// An `IBStream` over bytes in memory.
pub struct Stream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for Stream {
    type Interfaces = (IBStream,);
}

impl Stream {
    pub fn new(data: Vec<u8>) -> ComWrapper<Stream> {
        ComWrapper::new(Stream {
            inner: Mutex::new((data, 0)),
        })
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.inner.lock().unwrap().0.clone()
    }

    pub fn rewind(&self) {
        self.inner.lock().unwrap().1 = 0;
    }
}

impl IBStreamTrait for Stream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        numBytes: int32,
        numBytesRead: *mut int32,
    ) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = (numBytes.max(0) as usize).min(data.len().saturating_sub(*pos));
        // SAFETY: the plugin's buffer holds numBytes bytes
        unsafe {
            std::ptr::copy_nonoverlapping(data[*pos..].as_ptr(), buffer as *mut u8, n);
            if !numBytesRead.is_null() {
                *numBytesRead = n as int32;
            }
        }
        *pos += n;
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut c_void,
        numBytes: int32,
        numBytesWritten: *mut int32,
    ) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let (data, pos) = &mut *g;
        let n = numBytes.max(0) as usize;
        // SAFETY: the plugin's buffer holds numBytes bytes
        let src = unsafe { std::slice::from_raw_parts(buffer as *const u8, n) };
        if *pos + n > data.len() {
            data.resize(*pos + n, 0);
        }
        data[*pos..*pos + n].copy_from_slice(src);
        *pos += n;
        if !numBytesWritten.is_null() {
            // SAFETY: out pointer from the plugin
            unsafe { *numBytesWritten = n as int32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let mut g = self.inner.lock().unwrap();
        let len = g.0.len() as i64;
        let base = match mode as u32 {
            0 => 0,          // kIBSeekSet
            1 => g.1 as i64, // kIBSeekCur
            2 => len,        // kIBSeekEnd
            _ => return kInvalidArgument,
        };
        let new = (base + pos).clamp(0, len);
        g.1 = new as usize;
        if !result.is_null() {
            // SAFETY: out pointer from the plugin
            unsafe { *result = new };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            // SAFETY: out pointer from the plugin
            unsafe { *pos = self.inner.lock().unwrap().1 as int64 };
        }
        kResultOk
    }
}

// ------------------------------------------------------------------------------- component handler

/// Parameter edits from the plugin's own editor, by parameter index: the latest normalized
/// value and a dirty flag, read lock-free by the audio thread.
pub struct EditInbox {
    pub ids: Vec<ParamID>,
    pub values: Vec<AtomicU64>,
    pub dirty: Vec<AtomicBool>,
    pub any: AtomicBool,
    pub restart: AtomicBool,
    index: HashMap<ParamID, usize>,
}

impl EditInbox {
    pub fn new(ids: Vec<ParamID>) -> Self {
        let index = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        Self {
            values: ids.iter().map(|_| AtomicU64::new(0)).collect(),
            dirty: ids.iter().map(|_| AtomicBool::new(false)).collect(),
            ids,
            any: AtomicBool::new(false),
            restart: AtomicBool::new(false),
            index,
        }
    }
}

pub struct Handler {
    pub inbox: std::sync::Arc<EditInbox>,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, valueNormalized: ParamValue) -> tresult {
        if let Some(i) = self.inbox.index.get(&id) {
            self.inbox.values[*i].store(valueNormalized.to_bits(), Release);
            self.inbox.dirty[*i].store(true, Release);
            self.inbox.any.store(true, Release);
        }
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        self.inbox.restart.store(true, Relaxed);
        kResultOk
    }
}

// ------------------------------------------------------------------------------- plug frame

pub struct Frame {
    pub size: Mutex<Option<(i32, i32)>>,
}

impl Class for Frame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for Frame {
    unsafe fn resizeView(&self, view: *mut IPlugView, newSize: *mut ViewRect) -> tresult {
        if newSize.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the plugin passes its view and the size it wants
        let r = unsafe { *newSize };
        *self.size.lock().unwrap() = Some((r.right - r.left, r.bottom - r.top));
        if let Some(v) = unsafe { vst3::ComRef::<IPlugView>::from_raw(view) } {
            // SAFETY: as above
            unsafe { v.onSize(newSize) };
        }
        kResultOk
    }
}

// ------------------------------------------------------------------------------- real time

/// One parameter's points in a block (fixed capacity).
pub struct Queue {
    inner: UnsafeCell<QueueData>,
}

pub struct QueueData {
    pub id: ParamID,
    pub points: [(i32, f64); POINTS],
    pub len: usize,
}

pub const POINTS: usize = 64;
pub const QUEUES: usize = 128;
pub const EVENTS: usize = 1024;

// SAFETY: only the audio thread (and the plugin inside `process`, on that thread) touches it.
unsafe impl Sync for Queue {}
unsafe impl Send for Queue {}

impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}

impl Queue {
    #[allow(clippy::mut_from_ref)]
    fn data(&self) -> &mut QueueData {
        // SAFETY: see the Sync impl: one thread at a time
        unsafe { &mut *self.inner.get() }
    }
}

impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.data().id
    }
    unsafe fn getPointCount(&self) -> int32 {
        self.data().len as int32
    }
    unsafe fn getPoint(
        &self,
        index: int32,
        sampleOffset: *mut int32,
        value: *mut ParamValue,
    ) -> tresult {
        let d = self.data();
        match d
            .points
            .get(index as usize)
            .filter(|_| (index as usize) < d.len)
        {
            Some((o, v)) => {
                // SAFETY: out pointers from the plugin
                unsafe {
                    *sampleOffset = *o;
                    *value = *v;
                }
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn addPoint(
        &self,
        sampleOffset: int32,
        value: ParamValue,
        index: *mut int32,
    ) -> tresult {
        let d = self.data();
        if d.len >= POINTS {
            return kResultFalse;
        }
        d.points[d.len] = (sampleOffset, value);
        if !index.is_null() {
            // SAFETY: out pointer from the plugin
            unsafe { *index = d.len as int32 };
        }
        d.len += 1;
        kResultOk
    }
}

/// `IParameterChanges` over [`QUEUES`] fixed queues.
pub struct Changes {
    queues: Vec<ComWrapper<Queue>>,
    ptrs: Vec<*mut IParamValueQueue>,
    used: UnsafeCell<usize>,
}

// SAFETY: as for Queue.
unsafe impl Sync for Changes {}
unsafe impl Send for Changes {}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl Changes {
    /// Allocates (off the audio thread).
    pub fn new() -> ComWrapper<Changes> {
        let queues: Vec<ComWrapper<Queue>> = (0..QUEUES)
            .map(|_| {
                ComWrapper::new(Queue {
                    inner: UnsafeCell::new(QueueData {
                        id: 0,
                        points: [(0, 0.0); POINTS],
                        len: 0,
                    }),
                })
            })
            .collect();
        let ptrs = queues
            .iter()
            .map(|q| q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr())
            .collect();
        ComWrapper::new(Changes {
            queues,
            ptrs,
            used: UnsafeCell::new(0),
        })
    }

    #[allow(clippy::mut_from_ref)]
    fn used(&self) -> &mut usize {
        // SAFETY: one thread at a time
        unsafe { &mut *self.used.get() }
    }

    pub fn clear(&self) {
        *self.used() = 0;
    }

    fn queue_for(&self, id: ParamID) -> Option<usize> {
        let n = *self.used();
        if let Some(i) = (0..n).find(|i| self.queues[*i].data().id == id) {
            return Some(i);
        }
        if n >= QUEUES {
            return None;
        }
        let d = self.queues[n].data();
        d.id = id;
        d.len = 0;
        *self.used() = n + 1;
        Some(n)
    }

    /// Adds a point (real-time; dropped when full).
    pub fn push(&self, id: ParamID, offset: i32, value: f64) {
        if let Some(q) = self.queue_for(id) {
            let d = self.queues[q].data();
            // points must be in time order: a point at the same offset replaces the last one
            if d.len > 0 && d.points[d.len - 1].0 >= offset {
                d.points[d.len - 1] = (offset.max(d.points[d.len - 1].0), value);
            } else if d.len < POINTS {
                d.points[d.len] = (offset, value);
                d.len += 1;
            }
        }
    }

    /// The last value of every queue (what the plugin reported).
    pub fn last_values(&self, mut f: impl FnMut(ParamID, f64)) {
        for q in &self.queues[..*self.used()] {
            let d = q.data();
            if d.len > 0 {
                f(d.id, d.points[d.len - 1].1);
            }
        }
    }
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        *self.used() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if (index as usize) < *self.used() {
            self.ptrs[index as usize]
        } else {
            std::ptr::null_mut()
        }
    }
    unsafe fn addParameterData(
        &self,
        id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        // SAFETY: the plugin passes an id
        let id = unsafe { *id };
        match self.queue_for(id) {
            Some(q) => {
                if !index.is_null() {
                    // SAFETY: out pointer from the plugin
                    unsafe { *index = q as int32 };
                }
                self.ptrs[q]
            }
            None => std::ptr::null_mut(),
        }
    }
}

/// `IEventList` over a fixed array.
pub struct Events {
    inner: UnsafeCell<(Vec<Event>, usize)>,
}

// SAFETY: as for Queue.
unsafe impl Sync for Events {}
unsafe impl Send for Events {}

impl Class for Events {
    type Interfaces = (IEventList,);
}

impl Events {
    pub fn new() -> ComWrapper<Events> {
        // SAFETY: Event is plain data; all-zero is a valid value
        let zero: Event = unsafe { std::mem::zeroed() };
        ComWrapper::new(Events {
            inner: UnsafeCell::new((vec![zero; EVENTS], 0)),
        })
    }

    #[allow(clippy::mut_from_ref)]
    fn data(&self) -> &mut (Vec<Event>, usize) {
        // SAFETY: one thread at a time
        unsafe { &mut *self.inner.get() }
    }

    pub fn clear(&self) {
        self.data().1 = 0;
    }

    pub fn push(&self, e: Event) {
        let d = self.data();
        if d.1 < d.0.len() {
            d.0[d.1] = e;
            d.1 += 1;
        }
    }
}

impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> int32 {
        self.data().1 as int32
    }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        let d = self.data();
        if index < 0 || index as usize >= d.1 {
            return kInvalidArgument;
        }
        // SAFETY: out pointer from the plugin
        unsafe { *e = d.0[index as usize] };
        kResultOk
    }
    unsafe fn addEvent(&self, e: *mut Event) -> tresult {
        // SAFETY: the plugin passes an event
        self.push(unsafe { *e });
        kResultOk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3::ComPtr;

    #[test]
    fn streams_read_write_seek() {
        let s = Stream::new(Vec::new());
        let p: ComPtr<IBStream> = s.to_com_ptr().unwrap();
        let mut n = 0;
        let mut data = *b"hello";
        unsafe {
            p.write(data.as_mut_ptr() as *mut c_void, 5, &mut n);
            assert_eq!(n, 5);
            let mut at = 0;
            p.seek(1, 0, &mut at);
            assert_eq!(at, 1);
            let mut buf = [0u8; 8];
            p.read(buf.as_mut_ptr() as *mut c_void, 8, &mut n);
            assert_eq!(&buf[..n as usize], b"ello");
        }
        assert_eq!(s.bytes(), b"hello");
    }

    #[test]
    fn parameter_changes_and_events_hold_points_without_reallocating() {
        let c = Changes::new();
        c.push(7, 0, 0.5);
        c.push(7, 10, 0.75);
        c.push(9, 3, 1.0);
        let p: ComPtr<IParameterChanges> = c.to_com_ptr().unwrap();
        unsafe {
            assert_eq!(p.getParameterCount(), 2);
            let q = vst3::ComRef::<IParamValueQueue>::from_raw(p.getParameterData(0)).unwrap();
            assert_eq!(q.getParameterId(), 7);
            assert_eq!(q.getPointCount(), 2);
            let (mut o, mut v) = (0, 0.0);
            q.getPoint(1, &mut o, &mut v);
            assert_eq!((o, v), (10, 0.75));
        }
        let mut seen = Vec::new();
        c.last_values(|id, v| seen.push((id, v)));
        assert_eq!(seen, vec![(7, 0.75), (9, 1.0)]);
        c.clear();
        unsafe { assert_eq!(p.getParameterCount(), 0) };

        let e = Events::new();
        let mut ev: Event = unsafe { std::mem::zeroed() };
        ev.sampleOffset = 5;
        e.push(ev);
        let l: ComPtr<IEventList> = e.to_com_ptr().unwrap();
        unsafe {
            assert_eq!(l.getEventCount(), 1);
            let mut out: Event = std::mem::zeroed();
            assert_eq!(l.getEvent(0, &mut out), kResultOk);
            assert_eq!(out.sampleOffset, 5);
        }
    }

    #[test]
    fn the_host_context_makes_messages_with_attributes() {
        let h = ComWrapper::new(HostApp);
        let p: ComPtr<IHostApplication> = h.to_com_ptr().unwrap();
        let mut obj = std::ptr::null_mut();
        let mut cid: TUID = IMessage::IID.map(|b| b as std::ffi::c_char);
        let mut iid = cid;
        unsafe {
            assert_eq!(p.createInstance(&mut cid, &mut iid, &mut obj), kResultOk);
            let m = vst3::ComPtr::<IMessage>::from_raw(obj as *mut IMessage).unwrap();
            m.setMessageID(c"hello".as_ptr());
            assert_eq!(CStr::from_ptr(m.getMessageID()), c"hello");
            let a = vst3::ComRef::<IAttributeList>::from_raw(m.getAttributes()).unwrap();
            a.setInt(c"x".as_ptr(), 42);
            let mut v = 0;
            a.getInt(c"x".as_ptr(), &mut v);
            assert_eq!(v, 42);
            let mut name: String128 = [0; 128];
            p.getName(&mut name);
            assert_eq!(read_tchars(&name), "Patternhaus");
        }
    }
}
