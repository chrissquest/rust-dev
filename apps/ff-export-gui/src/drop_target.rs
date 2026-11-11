//! OS file drops for the ROM box.
//!
//! Slint has no portable "file dropped on the window" input, and on Windows the
//! obvious workaround does not work either: the winit backend registers its own OLE
//! drop target with `RegisterDragDrop`, so Explorer delivers a dropped file through
//! OLE and never posts the legacy `WM_DROPFILES` message. winit turns that into a
//! `WindowEvent::DroppedFile`, which Slint does not forward.
//!
//! So this revokes winit's target and registers its own `IDropTarget`. The COM glue
//! is deliberately the smallest that OLE accepts: refcounting, a type check, and a
//! `Drop` that pulls `CF_HDROP` out of the data object and hands the first path to
//! the UI.

use std::cell::{Cell, RefCell};
use std::ffi::{c_void, OsString};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Timer, TimerMode};
use windows_sys::core::{GUID, HRESULT};
use windows_sys::Win32::Foundation::{HWND, POINTL, S_OK};
use windows_sys::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL};
use windows_sys::Win32::System::Ole::{CF_HDROP, RegisterDragDrop, RevokeDragDrop};
use windows_sys::Win32::UI::Shell::{DragFinish, DragQueryFileW, HDROP};

use crate::AppWindow;

const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
const E_POINTER: HRESULT = 0x8000_4003u32 as i32;
const DROPEFFECT_NONE: u32 = 0;
const DROPEFFECT_COPY: u32 = 1;

/// `IDataObject` is opaque here on purpose: only its vtable pointer is ever touched,
/// and windows-sys does not export the COM interfaces this version.
#[repr(C)]
struct IDataObject {
    lp_vtbl: *const c_void,
}

thread_local! {
    static TARGET: RefCell<Option<slint::Weak<AppWindow>>> = const { RefCell::new(None) };
    /// Kept for the life of the window so a stopped timer is never dropped from
    /// inside its own callback.
    static RETRY: RefCell<Option<Timer>> = const { RefCell::new(None) };
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Hooks the drop target, retrying briefly because the native window may only appear
/// once the event loop has run.
pub fn install_when_ready(ui: &AppWindow) {
    TARGET.with(|slot| *slot.borrow_mut() = Some(ui.as_weak()));

    if try_install() {
        eprintln!("drag and drop: enabled");
        return;
    }

    let timer = Timer::default();
    let attempts = std::rc::Rc::new(Cell::new(0u32));
    timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
        let installed = try_install();
        attempts.set(attempts.get() + 1);

        if installed || attempts.get() > 40 {
            RETRY.with(|slot| {
                if let Some(timer) = slot.borrow().as_ref() {
                    timer.stop();
                }
            });
            if installed {
                eprintln!("drag and drop: enabled");
            } else {
                eprintln!("warning: drag and drop unavailable (no native window handle)");
            }
        }
    });

    RETRY.with(|slot| *slot.borrow_mut() = Some(timer));
}

fn try_install() -> bool {
    if INSTALLED.with(Cell::get) {
        return true;
    }

    let Some(ui) = TARGET.with(|slot| slot.borrow().as_ref().and_then(|weak| weak.upgrade())) else {
        return false;
    };

    let provider = ui.window().window_handle();
    let Ok(handle) = provider.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return false;
    };
    let hwnd = win32.hwnd.get() as HWND;

    unsafe {
        // winit already registered a drop target for this window; OLE allows one, so
        // it has to go before ours can be registered.
        let _ = RevokeDragDrop(hwnd);

        let target = Box::into_raw(Box::new(FileDropTarget {
            vtbl: &VTABLE,
            refs: AtomicUsize::new(1),
        }));

        if RegisterDragDrop(hwnd, target.cast()) == S_OK {
            INSTALLED.with(|installed| installed.set(true));
            true
        } else {
            false
        }
    }
}

fn accept(path: PathBuf) {
    let Some(ui) = TARGET.with(|slot| slot.borrow().as_ref().and_then(|weak| weak.upgrade())) else {
        return;
    };
    crate::accept_rom(&ui, &path);
}

// ---------------------------------------------------------------------------
// The COM object
// ---------------------------------------------------------------------------

#[repr(C)]
struct FileDropTarget {
    vtbl: *const IDropTargetVtbl,
    refs: AtomicUsize,
}

#[repr(C)]
struct IDropTargetVtbl {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    drag_enter: unsafe extern "system" fn(
        *mut c_void,
        *const IDataObject,
        u32,
        *const POINTL,
        *mut u32,
    ) -> HRESULT,
    drag_over: unsafe extern "system" fn(*mut c_void, u32, *const POINTL, *mut u32) -> HRESULT,
    drag_leave: unsafe extern "system" fn(*mut c_void) -> HRESULT,
    drop: unsafe extern "system" fn(
        *mut c_void,
        *const IDataObject,
        u32,
        *const POINTL,
        *mut u32,
    ) -> HRESULT,
}

/// The first four slots of every COM vtable; enough to call `GetData` on the data
/// object without redefining all of `IDataObject`.
#[repr(C)]
struct IDataObjectVtblPrefix {
    query_interface:
        unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_data: unsafe extern "system" fn(
        *mut IDataObject,
        *const FORMATETC,
        *mut STGMEDIUM,
    ) -> HRESULT,
}

static VTABLE: IDropTargetVtbl = IDropTargetVtbl {
    query_interface,
    add_ref,
    release,
    drag_enter,
    drag_over,
    drag_leave,
    drop,
};

unsafe extern "system" fn query_interface(
    this: *mut c_void,
    riid: *const GUID,
    object: *mut *mut c_void,
) -> HRESULT {
    if riid.is_null() || object.is_null() {
        return E_POINTER;
    }

    // IUnknown {00000000-0000-0000-C000-000000000046}
    // IDropTarget {00000122-0000-0000-C000-000000000046}
    let wanted = unsafe { *riid };
    let well_known = wanted.data2 == 0
        && wanted.data3 == 0
        && wanted.data4 == [0xC0, 0, 0, 0, 0, 0, 0, 0x46];

    if well_known && (wanted.data1 == 0 || wanted.data1 == 0x122) {
        unsafe { add_ref(this) };
        unsafe { *object = this };
        S_OK
    } else {
        unsafe { *object = ptr::null_mut() };
        E_NOINTERFACE
    }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    let target = unsafe { &*(this as *const FileDropTarget) };
    target.refs.fetch_add(1, Ordering::SeqCst) as u32 + 1
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let target = unsafe { &*(this as *const FileDropTarget) };
    let remaining = target.refs.fetch_sub(1, Ordering::SeqCst) - 1;
    // The initial reference is held for the life of the window, so this never
    // reaches zero and the target is not freed.
    remaining as u32
}

unsafe extern "system" fn drag_enter(
    _this: *mut c_void,
    data: *const IDataObject,
    _keys: u32,
    _point: *const POINTL,
    effect: *mut u32,
) -> HRESULT {
    let accepted = unsafe { first_path(data) }.is_some();
    if !effect.is_null() {
        unsafe { *effect = if accepted { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
    }
    S_OK
}

unsafe extern "system" fn drag_over(
    _this: *mut c_void,
    _keys: u32,
    _point: *const POINTL,
    effect: *mut u32,
) -> HRESULT {
    if !effect.is_null() {
        unsafe { *effect = DROPEFFECT_COPY };
    }
    S_OK
}

unsafe extern "system" fn drag_leave(_this: *mut c_void) -> HRESULT {
    S_OK
}

unsafe extern "system" fn drop(
    _this: *mut c_void,
    data: *const IDataObject,
    _keys: u32,
    _point: *const POINTL,
    effect: *mut u32,
) -> HRESULT {
    match unsafe { first_path(data) } {
        Some(path) => {
            accept(path);
            if !effect.is_null() {
                unsafe { *effect = DROPEFFECT_COPY };
            }
        }
        None => {
            if !effect.is_null() {
                unsafe { *effect = DROPEFFECT_NONE };
            }
        }
    }
    S_OK
}

/// The first path in a `CF_HDROP` data object.
unsafe fn first_path(data: *const IDataObject) -> Option<PathBuf> {
    if data.is_null() {
        return None;
    }

    let format = FORMATETC {
        cfFormat: CF_HDROP,
        ptd: ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT,
        lindex: -1,
        tymed: TYMED_HGLOBAL as u32,
    };

    let mut medium: STGMEDIUM = unsafe { std::mem::zeroed() };
    let vtbl = unsafe { (*data).lp_vtbl as *const IDataObjectVtblPrefix };
    let get_data = unsafe { (*vtbl).get_data };

    if unsafe { get_data(data as *mut _, &format, &mut medium) } < 0 {
        // Not a file drop (text, an image, ...): nothing to do.
        return None;
    }

    let drop_handle = unsafe { medium.u.hGlobal as HDROP };

    // 0xFFFFFFFF asks for the count rather than a path.
    let count = unsafe { DragQueryFileW(drop_handle, 0xFFFF_FFFF, ptr::null_mut(), 0) };
    if count == 0 {
        unsafe { DragFinish(drop_handle) };
        return None;
    }

    let length = unsafe { DragQueryFileW(drop_handle, 0, ptr::null_mut(), 0) } as usize;
    let mut buffer = vec![0u16; length + 1];
    let written = unsafe {
        DragQueryFileW(drop_handle, 0, buffer.as_mut_ptr(), buffer.len() as u32)
    } as usize;

    unsafe { DragFinish(drop_handle) };

    if written == 0 {
        return None;
    }
    buffer.truncate(written);

    Some(PathBuf::from(OsString::from_wide(&buffer)))
}
