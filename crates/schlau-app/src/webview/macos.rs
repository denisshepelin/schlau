//! A WKWebView laid over the reading pane for HTML that the native reader can't draw faithfully.
//!
//! It is locked down three times over: the document carries a CSP that allows no remote loads
//! (`schlau-html` writes it), a WebKit content rule list blocks every http(s) and ws(s) request at
//! the network layer, and JavaScript is off. Storage is non-persistent, so nothing survives a
//! message. Every navigation except the initial document load is cancelled; a clicked link opens
//! in the default browser instead.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSView, NSWorkspace};
use objc2_foundation::{NSError, NSObject, NSObjectNSKeyValueCoding, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate,
    WKNavigationType, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore,
};
use raw_window_handle::RawWindowHandle;

const BLOCK_REMOTE_RULES: &str = r#"[
  {"trigger": {"url-filter": "^https?://"}, "action": {"type": "block"}},
  {"trigger": {"url-filter": "^wss?://"}, "action": {"type": "block"}}
]"#;

struct Host {
    parent: Retained<NSView>,
    web: Option<Retained<WKWebView>>,
    delegate: Option<Retained<Delegate>>,
    rules: Option<Retained<WKContentRuleList>>,
    shown: Option<String>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SchlauNavigationDelegate"]
    #[ivars = ()]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web_view: &WKWebView,
            action: &WKNavigationAction,
            decision: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let url = unsafe { action.request().URL() };
            let scheme = url.as_ref().and_then(|u| u.scheme()).map(|s| s.to_string()).unwrap_or_default();
            let link = unsafe { action.navigationType() } == WKNavigationType::LinkActivated;
            let policy = match (link, scheme.as_str()) {
                (false, "about") => WKNavigationActionPolicy::Allow,
                (true, "http" | "https" | "mailto") => {
                    if let Some(url) = url {
                        NSWorkspace::sharedWorkspace().openURL(&url);
                    }
                    WKNavigationActionPolicy::Cancel
                }
                _ => WKNavigationActionPolicy::Cancel,
            };
            decision.call((policy,));
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Delegate> {
        let this = Delegate::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Remembers the window's content view. Call from `App::on_window`.
pub fn attach(handle: RawWindowHandle) {
    let RawWindowHandle::AppKit(appkit) = handle else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };
    let view: NonNull<NSView> = appkit.ns_view.cast();
    let parent = unsafe { Retained::retain(view.as_ptr()) };
    let Some(parent) = parent else { return };
    HOST.with(|host| {
        *host.borrow_mut() = Some(Host { parent, web: None, delegate: None, rules: None, shown: None });
    });
    compile_rules(mtm);
}

fn compile_rules(mtm: MainThreadMarker) {
    let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(mtm) }) else { return };
    let handler = RcBlock::new(|list: *mut WKContentRuleList, _error: *mut NSError| {
        let Some(list) = (unsafe { Retained::retain(list) }) else { return };
        HOST.with(|host| {
            if let Some(host) = host.borrow_mut().as_mut() {
                if let Some(web) = &host.web {
                    unsafe { web.configuration().userContentController().addContentRuleList(&list) };
                }
                host.rules = Some(list);
            }
        });
    });
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(&NSString::from_str("schlau-block-remote")),
            Some(&NSString::from_str(BLOCK_REMOTE_RULES)),
            Some(&handler),
        );
    }
}

fn make_web_view(host: &mut Host, mtm: MainThreadMarker) -> Retained<WKWebView> {
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe {
        config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
        config.defaultWebpagePreferences().setAllowsContentJavaScript(false);
        if let Some(rules) = &host.rules {
            config.userContentController().addContentRuleList(rules);
        }
    }
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 10.0));
    let web = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config) };
    let delegate = Delegate::new(mtm);
    unsafe {
        web.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        web.setValue_forKey(Some(&objc2_foundation::NSNumber::new_bool(false)), &NSString::from_str("drawsBackground"));
    }
    host.parent.addSubview(&web);
    host.delegate = Some(delegate);
    web
}

/// Shows `document` in the web view at `rect` (window points, top-left origin).
pub fn show(document: &str, rect: [f32; 4]) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    HOST.with(|host| {
        let mut host = host.borrow_mut();
        let Some(host) = host.as_mut() else { return };
        let web = match &host.web {
            Some(web) => web.clone(),
            None => {
                let web = make_web_view(host, mtm);
                host.web = Some(web.clone());
                web
            }
        };
        place(&host.parent, &web, rect);
        web.setHidden(false);
        if host.shown.as_deref() != Some(document) {
            unsafe { web.loadHTMLString_baseURL(&NSString::from_str(document), None) };
            host.shown = Some(document.to_string());
        }
    });
}

pub fn hide() {
    HOST.with(|host| {
        if let Some(host) = host.borrow_mut().as_mut()
            && let Some(web) = &host.web
        {
            web.setHidden(true);
            unsafe { web.loadHTMLString_baseURL(&NSString::from_str(""), None) };
            host.shown = None;
        }
    });
}

fn place(parent: &NSView, web: &WKWebView, [x, y, w, h]: [f32; 4]) {
    let bounds = parent.bounds();
    let top = if parent.isFlipped() { y as f64 } else { bounds.size.height - (y + h) as f64 };
    web.setFrame(NSRect::new(NSPoint::new(x as f64, top), NSSize::new(w as f64, h as f64)));
}
