//! The system Quick Look panel over attachment files in the cache. AppKit hands the panel to the
//! first object in the key window's responder chain that accepts it, so the window's view and the
//! window learn the three controller methods when the window opens. The panel is ordered front
//! without becoming key: schlau's window keeps the keyboard, as Finder's does, and its keymap
//! closes and steps the panel.

use std::cell::RefCell;
use std::ffi::CStr;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSView, NSWindow};
use objc2_foundation::{NSInteger, NSObject, NSObjectProtocol, NSString, NSURL};
use objc2_quick_look_ui::{QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource};
use raw_window_handle::RawWindowHandle;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SchlauQuickLookSource"]
    #[ivars = RefCell<Vec<Retained<NSURL>>>]
    struct Source;

    unsafe impl NSObjectProtocol for Source {}

    unsafe impl QLPreviewPanelDataSource for Source {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn number_of_items(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            self.ivars().borrow().len() as NSInteger
        }

        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn item_at(&self, _panel: Option<&QLPreviewPanel>, index: NSInteger) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            let urls = self.ivars().borrow();
            usize::try_from(index).ok().and_then(|i| urls.get(i)).map(|url| ProtocolObject::from_retained(url.clone()))
        }
    }
);

impl Source {
    fn new(mtm: MainThreadMarker) -> Retained<Source> {
        let this = Source::alloc(mtm).set_ivars(RefCell::new(Vec::new()));
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    static SOURCE: RefCell<Option<Retained<Source>>> = const { RefCell::new(None) };
}

fn source(mtm: MainThreadMarker) -> Retained<Source> {
    SOURCE.with(|s| s.borrow_mut().get_or_insert_with(|| Source::new(mtm)).clone())
}

unsafe extern "C-unwind" fn accepts_control(_this: *mut AnyObject, _cmd: Sel, _panel: *mut AnyObject) -> Bool {
    Bool::YES
}

unsafe extern "C-unwind" fn begin_control(_this: *mut AnyObject, _cmd: Sel, panel: *mut QLPreviewPanel) {
    let (Some(mtm), Some(panel)) = (MainThreadMarker::new(), unsafe { panel.as_ref() }) else { return };
    let source = source(mtm);
    unsafe { panel.setDataSource(Some(ProtocolObject::from_ref(&*source))) };
}

unsafe extern "C-unwind" fn end_control(_this: *mut AnyObject, _cmd: Sel, panel: *mut QLPreviewPanel) {
    if let Some(panel) = unsafe { panel.as_ref() } {
        unsafe { panel.setDataSource(None) };
    }
}

#[cfg(target_arch = "aarch64")]
const ACCEPTS_TYPES: &CStr = c"B@:@";
#[cfg(not(target_arch = "aarch64"))]
const ACCEPTS_TYPES: &CStr = c"c@:@";
const CONTROL_TYPES: &CStr = c"v@:@";

/// Teaches `class` to control the panel. Classes that already answer keep their own methods.
fn adopt(class: &AnyClass) {
    let class = class as *const AnyClass as *mut AnyClass;
    let methods: [(Sel, Imp, &CStr); 3] = unsafe {
        [
            (sel!(acceptsPreviewPanelControl:), std::mem::transmute::<*const (), Imp>(accepts_control as *const ()), ACCEPTS_TYPES),
            (sel!(beginPreviewPanelControl:), std::mem::transmute::<*const (), Imp>(begin_control as *const ()), CONTROL_TYPES),
            (sel!(endPreviewPanelControl:), std::mem::transmute::<*const (), Imp>(end_control as *const ()), CONTROL_TYPES),
        ]
    };
    methods.into_iter().for_each(|(name, imp, types)| {
        unsafe { objc2::ffi::class_addMethod(class, name, imp, types.as_ptr()) };
    });
}

/// Lets the window that holds `handle`'s view control the Quick Look panel.
pub fn attach(handle: RawWindowHandle) {
    let RawWindowHandle::AppKit(handle) = handle else { return };
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    adopt(view.class());
    if let Some(window) = view.window() {
        adopt(window.class());
    }
}

fn panel() -> Option<Retained<QLPreviewPanel>> {
    let mtm = MainThreadMarker::new()?;
    unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }
}

fn existing_panel() -> Option<Retained<QLPreviewPanel>> {
    let mtm = MainThreadMarker::new()?;
    unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) }.then(panel).flatten()
}

/// Shows `files` in the panel, starting at `index`.
pub fn show(files: &[PathBuf], index: usize) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let urls: Vec<Retained<NSURL>> =
        files.iter().filter_map(|f| f.to_str()).map(|f| NSURL::fileURLWithPath(&NSString::from_str(f))).collect();
    if urls.is_empty() {
        return;
    }
    *source(mtm).ivars().borrow_mut() = urls;
    let Some(panel) = panel() else { return };
    unsafe {
        panel.updateController();
        panel.reloadData();
        panel.setCurrentPreviewItemIndex(index as NSInteger);
    }
    let window: &NSWindow = &panel;
    window.orderFront(None);
}

pub fn is_open() -> bool {
    existing_panel().is_some_and(|p| {
        let window: &NSWindow = &p;
        window.isVisible()
    })
}

pub fn close() {
    if let Some(panel) = existing_panel() {
        let window: &NSWindow = &panel;
        window.orderOut(None);
    }
}

/// Moves `delta` files on, wrapping.
pub fn step(delta: isize) {
    let (Some(panel), Some(mtm)) = (existing_panel(), MainThreadMarker::new()) else { return };
    let count = source(mtm).ivars().borrow().len() as isize;
    if count > 0 {
        let current = unsafe { panel.currentPreviewItemIndex() };
        unsafe { panel.setCurrentPreviewItemIndex((current + delta).rem_euclid(count) as NSInteger) };
    }
}

/// The file the panel shows.
pub fn current() -> Option<PathBuf> {
    let (panel, mtm) = (existing_panel()?, MainThreadMarker::new()?);
    let index = usize::try_from(unsafe { panel.currentPreviewItemIndex() }).ok()?;
    let url = source(mtm).ivars().borrow().get(index)?.clone();
    url.path().map(|p| PathBuf::from(p.to_string()))
}
