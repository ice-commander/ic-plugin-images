#![allow(clippy::not_unsafe_ptr_arg_deref)]

use ic_plugin_api::{
    check_host, needs_up_to, HostCheck, IcBytes, IcFsSource, IcHost, IcViewVTable, IcViewerVTable,
    IC_ABI_VERSION, IC_ERR_HOST_TOO_OLD, IC_ERR_HOST_UNKNOWN, IC_ERR_INIT_FAILED, IC_HOST_CONSOLE,
    IC_OK, IC_OPEN_READ, IC_SEEK_END,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../version.rs"));

pub mod preview;

ic_plugin_api::declare_about!(
    "ic-imgviewer",
    "Pictures",
    plugins_version!(),
    "Shows pictures, the photograph inside a camera raw file included"
);

pub const ID: &str = "imgviewer";

const LOCALES: &[(&str, &str)] = &[
    ("en", include_str!("../locales/en.json")),
    ("ru", include_str!("../locales/ru.json")),
    ("uk", include_str!("../locales/uk.json")),
    ("be", include_str!("../locales/be.json")),
    ("pl", include_str!("../locales/pl.json")),
    ("cs", include_str!("../locales/cs.json")),
    ("sk", include_str!("../locales/sk.json")),
    ("de", include_str!("../locales/de.json")),
    ("es", include_str!("../locales/es.json")),
    ("it", include_str!("../locales/it.json")),
    ("fr", include_str!("../locales/fr.json")),
    ("ro", include_str!("../locales/ro.json")),
    ("hu", include_str!("../locales/hu.json")),
    ("bg", include_str!("../locales/bg.json")),
    ("sr", include_str!("../locales/sr.json")),
];

macro_rules! pictures {
    () => {
        ".png,.jpg,.jpeg,.gif,.bmp,.webp,.ico,.svg,.avif,.jxl,.tif,.tiff"
    };
}
macro_rules! raw {
    () => {
        ".nef,.cr2,.cr3,.arw,.dng,.raf,.orf,.rw2,.pef,.srw,.x3f"
    };
}

pub const PICTURES: &str = pictures!();

pub const RAW: &str = raw!();

pub const EXTENSIONS: &str = concat!(pictures!(), ",", raw!());

static HOST: AtomicUsize = AtomicUsize::new(0);

fn host() -> *const IcHost {
    HOST.load(Ordering::Relaxed) as *const IcHost
}

struct Showing {
    source: IcFsSource,
    pictures: Vec<String>,
    at: usize,
    size: u64,
    /// Backs the IcBytes viewer_content returns; replaced only when the window moves.
    drawn: Vec<u8>,
}

thread_local! {
    static SHOWING: RefCell<BTreeMap<u64, Showing>> = const { RefCell::new(BTreeMap::new()) };
    // ANSWER and DRAWN back the IcBytes returned to the host, valid until the next call.
    static ANSWER: RefCell<String> = const { RefCell::new(String::new()) };
    static DRAWN: RefCell<String> = const { RefCell::new(String::new()) };
}

fn asking_about(ctx: *const u8, len: u64) -> u64 {
    if ctx.is_null() || len == 0 {
        return 0;
    }
    let context = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(ctx, len as usize) });
    serde_json::from_str::<serde_json::Value>(&context)
        .ok()
        .and_then(|held| held.get("instance").and_then(|at| at.as_u64()))
        .unwrap_or(0)
}

fn length_of(source: IcFsSource, path: &str) -> u64 {
    let host = host();
    if host.is_null() {
        return 0;
    }
    let Ok(named) = CString::new(path) else {
        return 0;
    };
    let stream = unsafe { ((*host).fs_open)(source, named.as_ptr(), IC_OPEN_READ) };
    if stream.is_null() {
        return 0;
    }
    let end = unsafe { ((*host).fs_seek)(stream, 0, IC_SEEK_END) };
    unsafe { ((*host).fs_close)(stream) };
    end.max(0) as u64
}

fn claimed_by(list: &str, name: &str) -> bool {
    let lowered = name.to_lowercase();
    list.split(',').any(|claim| lowered.ends_with(claim.trim()))
}

pub fn is_a_picture(name: &str) -> bool {
    claimed_by(EXTENSIONS, name)
}

pub fn is_raw(name: &str) -> bool {
    claimed_by(RAW, name)
}

fn pictures_beside(source: IcFsSource) -> Vec<String> {
    let host = host();
    if host.is_null() {
        return Vec::new();
    }
    let Ok(here) = CString::new("/") else {
        return Vec::new();
    };
    let listing = unsafe { ((*host).fs_list)(source, here.as_ptr()) };
    let mut found: Vec<String> = listing
        .as_slice()
        .iter()
        .filter(|entry| entry.is_dir == 0)
        .map(|entry| entry.name_string())
        .filter(|name| is_a_picture(name))
        .collect();
    found.sort_by_key(|name| name.to_lowercase());
    found
}

// Whole file: kamadak-exif reads a TIFF whole, and the preview offset may point anywhere in it.
fn read_whole(source: IcFsSource, name: &str) -> Option<Vec<u8>> {
    let host = host();
    let named = CString::new(name).ok()?;
    if host.is_null() {
        return None;
    }
    let stream = unsafe { ((*host).fs_open)(source, named.as_ptr(), IC_OPEN_READ) };
    if stream.is_null() {
        return None;
    }
    let mut held = Vec::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = unsafe { ((*host).fs_read)(stream, buffer.as_mut_ptr(), buffer.len() as u64) };
        if read <= 0 {
            break;
        }
        held.extend_from_slice(&buffer[..read as usize]);
    }
    unsafe { ((*host).fs_close)(stream) };
    Some(held)
}

fn photograph_in(source: IcFsSource, name: &str) -> Vec<u8> {
    if !is_raw(name) {
        return Vec::new();
    }
    read_whole(source, name)
        .as_deref()
        .and_then(preview::extract_raw_thumbnail_from_bytes)
        .unwrap_or_default()
}

extern "C" fn viewer_open(
    instance: u64,
    source: IcFsSource,
    path: *const c_char,
    _user_data: *mut c_void,
) -> c_int {
    if path.is_null() {
        return IC_ERR_INIT_FAILED;
    }
    let opened = unsafe { CStr::from_ptr(path) }
        .to_string_lossy()
        .trim_matches('/')
        .to_string();
    let mut pictures = pictures_beside(source);
    if !pictures.iter().any(|name| *name == opened) {
        pictures = vec![opened.clone()];
    }
    let at = pictures
        .iter()
        .position(|name| *name == opened)
        .unwrap_or(0);
    let size = length_of(source, &pictures[at]);
    let drawn = photograph_in(source, &pictures[at]);
    SHOWING.with(|held| {
        held.borrow_mut().insert(
            instance,
            Showing {
                source,
                pictures,
                at,
                size,
                drawn,
            },
        )
    });
    IC_OK
}

extern "C" fn viewer_closed(instance: u64, _user_data: *mut c_void) {
    SHOWING.with(|held| held.borrow_mut().remove(&instance));
}

pub fn document_for(pictures: &[String], at: usize, size: u64, found: bool) -> String {
    let now = pictures.get(at).cloned().unwrap_or_default();
    let shown = if !is_raw(&now) {
        serde_json::json!({
            "t": "image",
            "id": "picture",
            "src": format!("file:{now}"),
            "fit": "contain",
            "zoom": true,
            "weight": 1
        })
    } else if found {
        // `at` in the name makes a request left over from the previous picture get nothing.
        serde_json::json!({
            "t": "image",
            "id": "picture",
            "src": format!("part:photo/{at}"),
            "fit": "contain",
            "zoom": true,
            "weight": 1
        })
    } else {
        serde_json::json!({
            "t": "text",
            "id": "picture",
            "role": "dim",
            "weight": 1,
            "wrap": true,
            "text": { "tr": "imgviewer.no_photograph",
                      "en": "There is no photograph inside this file." }
        })
    };
    serde_json::json!({
        "schema": 1,
        "data": {
            "can_back": at > 0,
            "can_go_on": at + 1 < pictures.len(),
        },
        "fields": [],
        "form": {
            "t": "view",
            "surface": "window",
            "spacing": 8,
            "padding": 8,
            "children": [
                shown,
                {
                    "t": "row",
                    "spacing": 8,
                    "children": [
                        { "t": "button", "id": "back", "label": { "literal": "\u{25c0}" },
                          "accel": "Left",
                          "sensitive": { "truthy": "data.can_back" },
                          "intent": { "do": "emit", "node": "back" } },
                        { "t": "button", "id": "on", "label": { "literal": "\u{25b6}" },
                          "accel": "Right",
                          "sensitive": { "truthy": "data.can_go_on" },
                          "intent": { "do": "emit", "node": "on" } },
                        {
                            "t": "text",
                            "id": "about",
                            "role": "dim",
                            "weight": 1,
                            "text": { "literal": format!(
                                "{now} — {} — {} / {}",
                                human(size), at + 1, pictures.len()
                            ) }
                        }
                    ]
                }
            ]
        }
    })
    .to_string()
}

fn wanted(node: &str, showing: &Showing) -> Option<usize> {
    match node {
        "back" => (showing.at > 0).then(|| showing.at - 1),
        "on" => (showing.at + 1 < showing.pictures.len()).then_some(showing.at + 1),
        _ => None,
    }
}

extern "C" fn viewer_event(raw: *const u8, len: u64, _user_data: *mut c_void) -> IcBytes {
    let asked = asking_about(raw, len);
    let event: serde_json::Value = if raw.is_null() || len == 0 {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(unsafe { std::slice::from_raw_parts(raw, len as usize) })
            .unwrap_or(serde_json::Value::Null)
    };
    let node = event
        .get("node")
        .and_then(|node| node.as_str())
        .unwrap_or("");
    let moved = SHOWING.with(|held| {
        let mut held = held.borrow_mut();
        let Some(showing) = held.get_mut(&asked) else {
            return false;
        };
        let Some(next) = wanted(node, showing) else {
            return false;
        };
        showing.at = next;
        showing.size = length_of(showing.source, &showing.pictures[next]);
        showing.drawn = photograph_in(showing.source, &showing.pictures[next]);
        true
    });
    ANSWER.with(|held| {
        let mut held = held.borrow_mut();
        *held = if moved {
            r#"{"redescribe":true}"#.to_string()
        } else {
            "{}".to_string()
        };
        IcBytes {
            data: held.as_ptr(),
            len: held.len() as u64,
        }
    })
}

extern "C" fn viewer_content(
    instance: u64,
    name: *const c_char,
    _user_data: *mut c_void,
) -> IcBytes {
    if name.is_null() {
        return IcBytes::EMPTY;
    }
    let asked = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    let Some(wanted): Option<usize> = asked.strip_prefix("photo/").and_then(|at| at.parse().ok())
    else {
        return IcBytes::EMPTY;
    };
    SHOWING.with(|held| {
        let held = held.borrow();
        let Some(showing) = held.get(&instance).filter(|showing| showing.at == wanted) else {
            return IcBytes::EMPTY;
        };
        if showing.drawn.is_empty() {
            return IcBytes::EMPTY;
        }
        IcBytes {
            data: showing.drawn.as_ptr(),
            len: showing.drawn.len() as u64,
        }
    })
}

pub fn human(size: u64) -> String {
    const STEPS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut held = size as f64;
    let mut step = 0;
    while held >= 1024.0 && step + 1 < STEPS.len() {
        held /= 1024.0;
        step += 1;
    }
    if step == 0 {
        format!("{size} {}", STEPS[0])
    } else {
        format!("{held:.1} {}", STEPS[step])
    }
}

extern "C" fn viewer_describe(ctx: *const u8, len: u64, _user_data: *mut c_void) -> IcBytes {
    let asked = asking_about(ctx, len);
    let drawn = SHOWING.with(|held| {
        let held = held.borrow();
        let showing = held.get(&asked).or_else(|| {
            // A context without "instance" is unambiguous only while one window is open.
            (asked == 0 && held.len() == 1).then(|| held.values().next())?
        })?;
        Some(document_for(
            &showing.pictures,
            showing.at,
            showing.size,
            !showing.drawn.is_empty(),
        ))
    });
    let Some(drawn) = drawn else {
        return IcBytes::EMPTY;
    };
    DRAWN.with(|held| {
        let mut held = held.borrow_mut();
        *held = drawn;
        IcBytes {
            data: held.as_ptr(),
            len: held.len() as u64,
        }
    })
}

pub fn view_vtable() -> IcViewVTable {
    IcViewVTable {
        struct_size: std::mem::size_of::<IcViewVTable>() as u32,
        describe: viewer_describe,
        on_event: Some(viewer_event),
        closed: None,
    }
}

/// The host copies both tables, so they may live on the caller's stack.
pub fn viewer_vtable(view: *const IcViewVTable) -> IcViewerVTable {
    IcViewerVTable {
        struct_size: std::mem::size_of::<IcViewerVTable>() as u32,
        view,
        open: viewer_open,
        closed: Some(viewer_closed),
        content: Some(viewer_content),
        closing: None,
        canvas_ready: None,
        canvas_draw: None,
        canvas_gone: None,
    }
}

fn in_console(kind: *const c_char) -> bool {
    !kind.is_null() && unsafe { CStr::from_ptr(kind) }.to_bytes() == IC_HOST_CONSOLE.as_bytes()
}

#[cfg_attr(feature = "export-abi", no_mangle)]
pub extern "C" fn ic_plugin_init(host: *const IcHost, kind: *const c_char) -> c_int {
    if in_console(kind) {
        return ic_plugin_api::IC_ERR_NOT_THIS_HOST;
    }
    match check_host(
        host,
        IC_ABI_VERSION,
        needs_up_to(std::mem::offset_of!(IcHost, register_viewer)),
    ) {
        HostCheck::Ok => {}
        HostCheck::WrongMagic => return IC_ERR_HOST_UNKNOWN,
        HostCheck::TooOld { .. } | HostCheck::Truncated { .. } => return IC_ERR_HOST_TOO_OLD,
    }
    HOST.store(host as usize, Ordering::Relaxed);
    for (language, catalogue) in LOCALES {
        let Ok(tag) = CString::new(*language) else {
            continue;
        };
        unsafe {
            ((*host).register_locales)(tag.as_ptr(), catalogue.as_ptr(), catalogue.len() as u64)
        };
    }
    let (Ok(id), Ok(extensions)) = (CString::new(ID), CString::new(EXTENSIONS)) else {
        return IC_ERR_INIT_FAILED;
    };
    let window = view_vtable();
    let viewer = viewer_vtable(&window);
    unsafe {
        ((*host).register_viewer)(
            id.as_ptr(),
            extensions.as_ptr(),
            0,
            &viewer,
            std::ptr::null_mut(),
        )
    }
}

#[cfg_attr(feature = "export-abi", no_mangle)]
pub extern "C" fn ic_plugin_shutdown() {
    SHOWING.with(|held| held.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_is_said_the_way_a_person_reads_it() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(999), "999 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(2_400_000), "2.3 MB");
    }

    #[test]
    fn two_pictures_open_at_once_do_not_answer_for_each_other() {
        let mut host = ic_plugin_api::testing::silent_host();
        host.fs_open = never_opens;
        HOST.store(&host as *const IcHost as usize, Ordering::Relaxed);

        let first = CString::new("first.png").expect("a path");
        let second = CString::new("second.png").expect("a path");
        assert_eq!(
            viewer_open(
                7,
                std::ptr::null_mut(),
                first.as_ptr(),
                std::ptr::null_mut()
            ),
            IC_OK
        );
        assert_eq!(
            viewer_open(
                9,
                std::ptr::null_mut(),
                second.as_ptr(),
                std::ptr::null_mut()
            ),
            IC_OK
        );

        assert!(described(7).contains("file:first.png"));
        assert!(described(9).contains("file:second.png"));

        viewer_closed(7, std::ptr::null_mut());
        assert!(
            described(7).is_empty(),
            "a window that was closed answers nothing"
        );
        assert!(
            described(9).contains("file:second.png"),
            "and the other is untouched"
        );
        viewer_closed(9, std::ptr::null_mut());
        HOST.store(0, Ordering::Relaxed);
    }

    extern "C" fn never_opens(_: IcFsSource, _: *const c_char, _: u32) -> ic_plugin_api::IcStream {
        std::ptr::null_mut()
    }

    fn described(instance: u64) -> String {
        let context = format!(r#"{{"host":{{"kind":"gtk"}},"instance":{instance}}}"#);
        let answered =
            viewer_describe(context.as_ptr(), context.len() as u64, std::ptr::null_mut());
        if answered.data.is_null() {
            return String::new();
        }
        String::from_utf8_lossy(unsafe {
            std::slice::from_raw_parts(answered.data, answered.len as usize)
        })
        .into_owned()
    }

    #[test]
    fn a_terminal_has_nothing_to_draw_a_picture_on() {
        let host = ic_plugin_api::testing::silent_host();
        let console = CString::new(IC_HOST_CONSOLE).expect("a kind");
        assert_ne!(ic_plugin_init(&host, console.as_ptr()), IC_OK);
    }
}

#[cfg(test)]
mod walking_the_folder {
    use super::*;

    #[test]
    fn only_pictures_are_walked_through() {
        assert!(is_a_picture("Holiday.JPG"));
        assert!(is_a_picture("scan.tiff"));
        assert!(!is_a_picture("notes.txt"));
        assert!(!is_a_picture("jpg"));
    }

    #[test]
    fn what_a_camera_wrote_is_walked_through_with_everything_else() {
        assert!(is_a_picture("DSC_0001.NEF"));
        assert!(is_a_picture("shot.cr3"));
        assert!(is_raw("DSC_0001.NEF"));
        assert!(!is_raw("Holiday.JPG"));
        assert!(!is_raw("notes.txt"));
    }

    // An extension in both halves would send an ordinary picture down the camera-file path.
    #[test]
    fn the_two_halves_of_the_list_do_not_overlap() {
        for claim in RAW.split(',') {
            assert!(
                !PICTURES.split(',').any(|other| other == claim),
                "{claim} is claimed twice"
            );
        }
        assert_eq!(
            EXTENSIONS.split(',').count(),
            PICTURES.split(',').count() + RAW.split(',').count()
        );
    }
}

#[cfg(test)]
mod showing_what_is_inside {
    use super::*;

    #[test]
    fn a_picture_is_named_as_a_file_and_a_photograph_as_a_part() {
        let pictures = vec!["Holiday.jpg".to_string(), "DSC_0001.NEF".to_string()];
        assert!(document_for(&pictures, 0, 10, false).contains("file:Holiday.jpg"));
        assert!(document_for(&pictures, 1, 10, true).contains("part:photo/1"));
    }

    #[test]
    fn a_camera_file_with_no_photograph_inside_says_so() {
        let pictures = vec!["DSC_0001.NEF".to_string()];
        let drawn = document_for(&pictures, 0, 10, false);
        assert!(!drawn.contains("part:photo"));
        assert!(drawn.contains("imgviewer.no_photograph"));
    }

    #[test]
    fn the_photograph_is_handed_over_for_the_picture_on_the_screen() {
        SHOWING.with(|held| {
            held.borrow_mut().insert(
                5,
                Showing {
                    source: std::ptr::null_mut(),
                    pictures: vec!["a.nef".to_string(), "b.nef".to_string()],
                    at: 1,
                    size: 10,
                    drawn: b"a jpeg".to_vec(),
                },
            )
        });
        let asked = CString::new("photo/1").expect("a name");
        let answered = viewer_content(5, asked.as_ptr(), std::ptr::null_mut());
        assert_eq!(
            unsafe { std::slice::from_raw_parts(answered.data, answered.len as usize) },
            b"a jpeg"
        );

        let stale = CString::new("photo/0").expect("a name");
        assert!(
            viewer_content(5, stale.as_ptr(), std::ptr::null_mut())
                .data
                .is_null(),
            "the window is not on that one, so there is nothing to give"
        );
        assert!(viewer_content(404, asked.as_ptr(), std::ptr::null_mut())
            .data
            .is_null());
        viewer_closed(5, std::ptr::null_mut());
    }
}
