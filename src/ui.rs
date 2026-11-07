use std::{
    collections::HashMap,
    iter::FromIterator,
    sync::{Arc, Mutex},
};

use sciter::Value;

use hbb_common::{
    allow_err,
    config::{LocalConfig, PeerConfig},
    log,
};

#[cfg(not(feature = "flutter"))]
use crate::ui_session_interface::Session;
use crate::{common::get_app_name, ipc, ui_interface::*};

mod cm;
#[cfg(feature = "inline")]
pub mod inline;
pub mod remote;

#[allow(dead_code)]
type Status = (i32, bool, i64, String);

lazy_static::lazy_static! {
    // stupid workaround for https://sciter.com/forums/topic/crash-on-latest-tis-mac-sdk-sometimes/
    static ref STUPID_VALUES: Mutex<Vec<Arc<Vec<Value>>>> = Default::default();
}

#[cfg(not(feature = "flutter"))]
lazy_static::lazy_static! {
    pub static ref CUR_SESSION: Arc<Mutex<Option<Session<remote::SciterHandler>>>> = Default::default();
}

struct UIHostHandler;

pub fn start(args: &mut [String]) {
    #[cfg(target_os = "macos")]
    crate::platform::delegate::show_dock();
    #[cfg(all(target_os = "linux", feature = "inline"))]
    {
        let app_dir = std::env::var("APPDIR").unwrap_or("".to_string());
        let mut so_path = "/usr/share/rustdesk/libsciter-gtk.so".to_owned();
        for (prefix, dir) in [
            ("", "/usr"),
            ("", "/app"),
            (&app_dir, "/usr"),
            (&app_dir, "/app"),
        ]
        .iter()
        {
            let path = format!("{prefix}{dir}/share/rustdesk/libsciter-gtk.so");
            if std::path::Path::new(&path).exists() {
                so_path = path;
                break;
            }
        }
        sciter::set_library(&so_path).ok();
    }
    #[cfg(windows)]
    // Check if there is a sciter.dll nearby.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let sciter_dll_path = parent.join("sciter.dll");
            if sciter_dll_path.exists() {
                // Try to set the sciter dll.
                let p = sciter_dll_path.to_string_lossy().to_string();
                log::debug!("Found dll:{}, \n {:?}", p, sciter::set_library(&p));
            }
        }
    }
    // https://github.com/c-smile/sciter-sdk/blob/master/include/sciter-x-types.h
    // https://github.com/rustdesk/rustdesk/issues/132#issuecomment-886069737
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::GfxLayer(
        sciter::GFX_LAYER::WARP
    )));
    use sciter::SCRIPT_RUNTIME_FEATURES::*;
    allow_err!(sciter::set_options(sciter::RuntimeOptions::ScriptFeatures(
        ALLOW_FILE_IO as u8 | ALLOW_SOCKET_IO as u8 | ALLOW_EVAL as u8 | ALLOW_SYSINFO as u8
    )));
    let mut frame = sciter::WindowBuilder::main_window().create();
    #[cfg(windows)]
    allow_err!(sciter::set_options(sciter::RuntimeOptions::UxTheming(true)));
    frame.set_title(&crate::get_app_name());
    #[cfg(target_os = "macos")]
    crate::platform::delegate::make_menubar(frame.get_host(), args.is_empty());
    #[cfg(windows)]
    crate::platform::try_set_window_foreground(frame.get_hwnd() as _);
    let page;
    if args.len() > 1 && args[0] == "--play" {
        args[0] = "--connect".to_owned();
        let path: std::path::PathBuf = (&args[1]).into();
        let id = path
            .file_stem()
            .map(|p| p.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_owned();
        args[1] = id;
    }
    if args.is_empty() {
        std::thread::spawn(move || check_zombie());
        crate::common::check_software_update();
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "index.html";
        // Start pulse audio local server.
        #[cfg(target_os = "linux")]
        std::thread::spawn(crate::ipc::start_pa);
    } else if args[0] == "--install" {
        frame.event_handler(UI {});
        frame.sciter_handler(UIHostHandler {});
        page = "install.html";
    } else if args[0] == "--cm" {
        frame.register_behavior("connection-manager", move || {
            Box::new(cm::SciterConnectionManager::new())
        });
        page = "cm.html";
        *cm::HIDE_CM.lock().unwrap() = crate::ipc::get_config("hide_cm")
            .ok()
            .flatten()
            .unwrap_or_default()
            == "true";
    } else if (args[0] == "--connect"
        || args[0] == "--file-transfer"
        || args[0] == "--port-forward"
        || args[0] == "--rdp")
        && args.len() > 1
    {
        #[cfg(windows)]
        {
            let hw = frame.get_host().get_hwnd();
            crate::platform::windows::enable_lowlevel_keyboard(hw as _);
        }
        let mut iter = args.iter();
        let Some(cmd) = iter.next() else {
            log::error!("Failed to get cmd arg");
            return;
        };
        let cmd = cmd.to_owned();
        let Some(id) = iter.next() else {
            log::error!("Failed to get id arg");
            return;
        };
        let id = id.to_owned();
        let pass = iter.next().unwrap_or(&"".to_owned()).clone();
        let args: Vec<String> = iter.map(|x| x.clone()).collect();
        frame.set_title(&id);
        frame.register_behavior("native-remote", move || {
            let handler =
                remote::SciterSession::new(cmd.clone(), id.clone(), pass.clone(), args.clone());
            #[cfg(not(feature = "flutter"))]
            {
                *CUR_SESSION.lock().unwrap() = Some(handler.inner());
            }
            Box::new(handler)
        });
        page = "remote.html";
    } else {
        log::error!("Wrong command: {:?}", args);
        return;
    }
    #[cfg(feature = "inline")]
    {
        let html = if page == "index.html" {
            inline::get_index()
        } else if page == "cm.html" {
            inline::get_cm()
        } else if page == "install.html" {
            inline::get_install()
        } else {
            inline::get_remote()
        };
        frame.load_html(html.as_bytes(), Some(page));
    }
    #[cfg(not(feature = "inline"))]
    frame.load_file(&format!(
        "file://{}/src/ui/{}",
        std::env::current_dir()
            .map(|c| c.display().to_string())
            .unwrap_or("".to_owned()),
        page
    ));
    let hide_cm = *cm::HIDE_CM.lock().unwrap();
    if !args.is_empty() && args[0] == "--cm" && hide_cm {
        // run_app calls expand(show) + run_loop, we use collapse(hide) + run_loop instead to create a hidden window
        frame.collapse(true);
        frame.run_loop();
        return;
    }
    frame.run_app();
}

struct UI {}

impl UI {
    fn recent_sessions_updated(&self) -> bool {
        recent_sessions_updated()
    }

    fn get_id(&self) -> String {
        ipc::get_id()
    }

    fn temporary_password(&mut self) -> String {
        temporary_password()
    }

    fn update_temporary_password(&self) {
        update_temporary_password()
    }

    fn set_permanent_password(&self, password: String) {
        let _ = set_permanent_password_with_result(password);
    }

    fn is_local_permanent_password_set(&self) -> bool {
        is_local_permanent_password_set()
    }

    fn is_permanent_password_set(&self) -> bool {
        is_permanent_password_set()
    }

    fn get_remote_id(&mut self) -> String {
        LocalConfig::get_remote_id()
    }

    fn set_remote_id(&mut self, id: String) {
        LocalConfig::set_remote_id(&id);
    }

    fn goto_install(&mut self) {
        goto_install();
    }

    fn install_me(&mut self, _options: String, _path: String) {
        install_me(_options, _path, false, false);
    }

    fn update_me(&self, _path: String) {
        update_me(_path);
    }

    fn run_without_install(&self) {
        run_without_install();
    }

    fn show_run_without_install(&self) -> bool {
        show_run_without_install()
    }

    fn get_license(&self) -> String {
        get_license()
    }

    fn get_option(&self, key: String) -> String {
        get_option(key)
    }

    fn get_local_option(&self, key: String) -> String {
        get_local_option(key)
    }

    fn set_local_option(&self, key: String, value: String) {
        set_local_option(key, value);
    }

    fn peer_has_password(&self, id: String) -> bool {
        peer_has_password(id)
    }

    fn forget_password(&self, id: String) {
        forget_password(id)
    }

    fn get_peer_option(&self, id: String, name: String) -> String {
        get_peer_option(id, name)
    }

    fn set_peer_option(&self, id: String, name: String, value: String) {
        set_peer_option(id, name, value)
    }

    fn using_public_server(&self) -> bool {
        crate::using_public_server()
    }

    fn is_incoming_only(&self) -> bool {
        hbb_common::config::is_incoming_only()
    }

    pub fn is_outgoing_only(&self) -> bool {
        hbb_common::config::is_outgoing_only()
    }

    pub fn is_custom_client(&self) -> bool {
        crate::common::is_custom_client()
    }

    pub fn is_disable_settings(&self) -> bool {
        hbb_common::config::is_disable_settings()
    }

    pub fn is_disable_account(&self) -> bool {
        hbb_common::config::is_disable_account()
    }

    pub fn is_disable_installation(&self) -> bool {
        hbb_common::config::is_disable_installation()
    }

    pub fn is_disable_ab(&self) -> bool {
        hbb_common::config::is_disable_ab()
    }

    fn get_options(&self) -> Value {
        let hashmap: HashMap<String, String> =
            serde_json::from_str(&get_options()).unwrap_or_default();
        let mut m = Value::map();
        for (k, v) in hashmap {
            m.set_item(k, v);
        }
        m
    }

    fn test_if_valid_server(&self, host: String, test_with_proxy: bool) -> String {
        test_if_valid_server(host, test_with_proxy)
    }

    fn get_sound_inputs(&self) -> Value {
        Value::from_iter(get_sound_inputs())
    }

    fn set_options(&self, v: Value) {
        let mut m = HashMap::new();
        for (k, v) in v.items() {
            if let Some(k) = k.as_string() {
                if let Some(v) = v.as_string() {
                    if !v.is_empty() {
                        m.insert(k, v);
                    }
                }
            }
        }
        set_options(m);
    }

    fn set_option(&self, key: String, value: String) {
        set_option(key, value);
    }

    fn install_path(&mut self) -> String {
        install_path()
    }

    fn install_options(&self) -> String {
        install_options()
    }

    fn get_socks(&self) -> Value {
        Value::from_iter(get_socks())
    }

    fn set_socks(&self, proxy: String, username: String, password: String) {
        set_socks(proxy, username, password)
    }

    fn is_installed(&self) -> bool {
        is_installed()
    }

    fn get_supported_privacy_mode_impls(&self) -> String {
        serde_json::to_string(&crate::privacy_mode::get_supported_privacy_mode_impl())
            .unwrap_or_default()
    }

    fn is_root(&self) -> bool {
        is_root()
    }

    fn is_release(&self) -> bool {
        #[cfg(not(debug_assertions))]
        return true;
        #[cfg(debug_assertions)]
        return false;
    }

    fn is_share_rdp(&self) -> bool {
        is_share_rdp()
    }

    fn set_share_rdp(&self, _enable: bool) {
        set_share_rdp(_enable);
    }

    fn is_installed_lower_version(&self) -> bool {
        is_installed_lower_version()
    }

    fn closing(&mut self, x: i32, y: i32, w: i32, h: i32) {
        crate::server::input_service::fix_key_down_timeout_at_exit();
        LocalConfig::set_size(x, y, w, h);
    }

    fn get_size(&mut self) -> Value {
        let s = LocalConfig::get_size();
        let mut v = Vec::new();
        v.push(s.0);
        v.push(s.1);
        v.push(s.2);
        v.push(s.3);
        Value::from_iter(v)
    }

    fn get_mouse_time(&self) -> f64 {
        get_mouse_time()
    }

    fn check_mouse_time(&self) {
        check_mouse_time()
    }

    fn get_connect_status(&mut self) -> Value {
        let mut v = Value::array(0);
        let x = get_connect_status();
        v.push(x.status_num);
        v.push(x.key_confirmed);
        v.push(x.id);
        v
    }

    #[inline]
    fn get_peer_value(id: String, p: PeerConfig) -> Value {
        let values = vec![
            id,
            p.info.username.clone(),
            p.info.hostname.clone(),
            p.info.platform.clone(),
            p.options.get("alias").unwrap_or(&"".to_owned()).to_owned(),
        ];
        Value::from_iter(values)
    }

    fn get_peer(&self, id: String) -> Value {
        let c = get_peer(id.clone());
        Self::get_peer_value(id, c)
    }

    fn get_fav(&self) -> Value {
        Value::from_iter(get_fav())
    }

    fn store_fav(&self, fav: Value) {
        let mut tmp = vec![];
        fav.values().for_each(|v| {
            if let Some(v) = v.as_string() {
                if !v.is_empty() {
                    tmp.push(v);
                }
            }
        });
        store_fav(tmp);
    }

    fn get_recent_sessions(&mut self) -> Value {
        // to-do: limit number of recent sessions, and remove old peer file
        let peers: Vec<Value> = PeerConfig::peers(None)
            .drain(..)
            .map(|p| Self::get_peer_value(p.0, p.2))
            .collect();
        Value::from_iter(peers)
    }

    fn get_icon(&mut self) -> String {
        get_icon()
    }

    fn remove_peer(&mut self, id: String) {
        PeerConfig::remove(&id);
    }

    fn remove_discovered(&mut self, id: String) {
        remove_discovered(id);
    }

    fn send_wol(&mut self, id: String) {
        crate::lan::send_wol(id)
    }

    fn new_remote(&mut self, id: String, remote_type: String, force_relay: bool) {
        new_remote(id, remote_type, force_relay)
    }

    fn is_process_trusted(&mut self, _prompt: bool) -> bool {
        is_process_trusted(_prompt)
    }

    fn is_can_screen_recording(&mut self, _prompt: bool) -> bool {
        is_can_screen_recording(_prompt)
    }

    fn is_installed_daemon(&mut self, _prompt: bool) -> bool {
        is_installed_daemon(_prompt)
    }

    fn get_error(&mut self) -> String {
        get_error()
    }

    fn is_login_wayland(&mut self) -> bool {
        is_login_wayland()
    }

    fn current_is_wayland(&mut self) -> bool {
        current_is_wayland()
    }

    fn get_software_update_url(&self) -> String {
        crate::SOFTWARE_UPDATE_URL.lock().unwrap().clone()
    }

    fn get_new_version(&self) -> String {
        get_new_version()
    }

    fn get_version(&self) -> String {
        get_version()
    }

    fn get_fingerprint(&self) -> String {
        get_fingerprint()
    }

    fn get_app_name(&self) -> String {
        get_app_name()
    }

    fn get_software_ext(&self) -> String {
        #[cfg(windows)]
        let p = "exe";
        #[cfg(target_os = "macos")]
        let p = "dmg";
        #[cfg(target_os = "linux")]
        let p = "deb";
        p.to_owned()
    }

    fn get_software_store_path(&self) -> String {
        let mut p = std::env::temp_dir();
        let name = crate::SOFTWARE_UPDATE_URL
            .lock()
            .unwrap()
            .split("/")
            .last()
            .map(|x| x.to_owned())
            .unwrap_or(crate::get_app_name());
        p.push(name);
        format!("{}.{}", p.to_string_lossy(), self.get_software_ext())
    }

    fn create_shortcut(&self, _id: String) {
        #[cfg(windows)]
        create_shortcut(_id)
    }

    fn discover(&self) {
        std::thread::spawn(move || {
            allow_err!(crate::lan::discover());
        });
    }

    fn get_lan_peers(&self) -> String {
        // let peers = get_lan_peers()
        //     .into_iter()
        //     .map(|mut peer| {
        //         (
        //             peer.remove("id").unwrap_or_default(),
        //             peer.remove("username").unwrap_or_default(),
        //             peer.remove("hostname").unwrap_or_default(),
        //             peer.remove("platform").unwrap_or_default(),
        //         )
        //     })
        //     .collect::<Vec<(String, String, String, String)>>();
        serde_json::to_string(&get_lan_peers()).unwrap_or_default()
    }

    fn get_uuid(&self) -> String {
        get_uuid()
    }

    fn open_url(&self, url: String) {
        #[cfg(windows)]
        let p = "explorer";
        #[cfg(target_os = "macos")]
        let p = "open";
        #[cfg(target_os = "linux")]
        let p = if std::path::Path::new("/usr/bin/firefox").exists() {
            "firefox"
        } else {
            "xdg-open"
        };
        allow_err!(std::process::Command::new(p).arg(url).spawn());
    }

    fn change_id(&self, id: String) {
        reset_async_job_status();
        let old_id = self.get_id();
        change_id_shared(id, old_id);
    }

    fn http_request(&self, url: String, method: String, body: Option<String>, header: String) {
        http_request(url, method, body, header)
    }

    fn post_request(&self, url: String, body: String, header: String) {
        post_request(url, body, header)
    }

    fn is_ok_change_id(&self) -> bool {
        hbb_common::machine_uid::get().is_ok()
    }

    fn get_async_job_status(&self) -> String {
        get_async_job_status()
    }

    fn get_http_status(&self, url: String) -> Option<String> {
        get_async_http_status(url)
    }

    fn t(&self, name: String) -> String {
        crate::client::translate(name)
    }

    fn is_xfce(&self) -> bool {
        crate::platform::is_xfce()
    }

    fn get_api_server(&self) -> String {
        get_api_server()
    }

    fn has_hwcodec(&self) -> bool {
        has_hwcodec()
    }

    fn has_vram(&self) -> bool {
        has_vram()
    }

    fn get_langs(&self) -> String {
        get_langs()
    }

    fn video_save_directory(&self, root: bool) -> String {
        video_save_directory(root)
    }

    fn handle_relay_id(&self, id: String) -> String {
        handle_relay_id(&id).to_owned()
    }

    fn get_login_device_info(&self) -> String {
        get_login_device_info_json()
    }

    fn support_remove_wallpaper(&self) -> bool {
        support_remove_wallpaper()
    }

    fn has_valid_2fa(&self) -> bool {
        has_valid_2fa()
    }

    fn generate2fa(&self) -> String {
        generate2fa()
    }

    pub fn verify2fa(&self, code: String) -> bool {
        verify2fa(code)
    }

    fn verify_login(&self, raw: String, id: String) -> bool {
        crate::verify_login(&raw, &id)
    }

    fn generate_2fa_img_src(&self, data: String) -> String {
        let v = qrcode_generator::to_png_to_vec(data, qrcode_generator::QrCodeEcc::Low, 128)
            .unwrap_or_default();
        let s = hbb_common::sodiumoxide::base64::encode(
            v,
            hbb_common::sodiumoxide::base64::Variant::Original,
        );
        format!("data:image/png;base64,{s}")
    }

    pub fn check_hwcodec(&self) {
        check_hwcodec()
    }

    fn is_option_fixed(&self, key: String) -> bool {
        crate::ui_interface::is_option_fixed(&key)
    }

    fn get_builtin_option(&self, key: String) -> String {
        crate::ui_interface::get_builtin_option(&key)
    }

    fn is_remote_modify_enabled_by_control_permissions(&self) -> String {
        match crate::ui_interface::is_remote_modify_enabled_by_control_permissions() {
            Some(true) => "true",
            Some(false) => "false",
            None => "",
        }
        .to_string()
    }
}

impl sciter::EventHandler for UI {
    sciter::dispatch_script_call! {
        fn t(String);
        fn get_api_server();
        fn is_xfce();
        fn using_public_server();
        fn is_custom_client();
        fn is_outgoing_only();
        fn is_incoming_only();
        fn is_disable_settings();
        fn is_disable_account();
        fn is_disable_installation();
        fn is_disable_ab();
        fn get_id();
        fn temporary_password();
        fn update_temporary_password();
        fn set_permanent_password(String);
        fn is_local_permanent_password_set();
        fn is_permanent_password_set();
        fn get_remote_id();
        fn set_remote_id(String);
        fn closing(i32, i32, i32, i32);
        fn get_size();
        fn new_remote(String, String, bool);
        fn send_wol(String);
        fn remove_peer(String);
        fn remove_discovered(String);
        fn get_connect_status();
        fn get_mouse_time();
        fn check_mouse_time();
        fn get_recent_sessions();
        fn get_peer(String);
        fn get_fav();
        fn store_fav(Value);
        fn recent_sessions_updated();
        fn get_icon();
        fn install_me(String, String);
        fn is_installed();
        fn get_supported_privacy_mode_impls();
        fn is_root();
        fn is_release();
        fn set_socks(String, String, String);
        fn get_socks();
        fn is_share_rdp();
        fn set_share_rdp(bool);
        fn is_installed_lower_version();
        fn install_path();
        fn install_options();
        fn goto_install();
        fn is_process_trusted(bool);
        fn is_can_screen_recording(bool);
        fn is_installed_daemon(bool);
        fn get_error();
        fn is_login_wayland();
        fn current_is_wayland();
        fn get_options();
        fn get_option(String);
        fn get_local_option(String);
        fn set_local_option(String, String);
        fn get_peer_option(String, String);
        fn peer_has_password(String);
        fn forget_password(String);
        fn set_peer_option(String, String, String);
        fn get_license();
        fn test_if_valid_server(String, bool);
        fn get_sound_inputs();
        fn set_options(Value);
        fn set_option(String, String);
        fn get_software_update_url();
        fn get_new_version();
        fn get_version();
        fn get_fingerprint();
        fn update_me(String);
        fn show_run_without_install();
        fn run_without_install();
        fn get_app_name();
        fn get_software_store_path();
        fn get_software_ext();
        fn open_url(String);
        fn change_id(String);
        fn get_async_job_status();
        fn post_request(String, String, String);
        fn is_ok_change_id();
        fn create_shortcut(String);
        fn discover();
        fn get_lan_peers();
        fn get_uuid();
        fn has_hwcodec();
        fn has_vram();
        fn get_langs();
        fn video_save_directory(bool);
        fn handle_relay_id(String);
        fn get_login_device_info();
        fn support_remove_wallpaper();
        fn has_valid_2fa();
        fn generate2fa();
        fn generate_2fa_img_src(String);
        fn verify2fa(String);
        fn check_hwcodec();
        fn verify_login(String, String);
        fn is_option_fixed(String);
        fn get_builtin_option(String);
        fn is_remote_modify_enabled_by_control_permissions();
    }
}

impl sciter::host::HostHandler for UIHostHandler {
    fn on_graphics_critical_failure(&mut self) {
        log::error!("Critical rendering error: e.g. DirectX gfx driver error. Most probably bad gfx drivers.");
    }
}

#[cfg(not(target_os = "linux"))]
fn get_sound_inputs() -> Vec<String> {
    let mut out = Vec::new();
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Ok(devices) = host.devices() {
        for device in devices {
            if device.default_input_config().is_err() {
                continue;
            }
            if let Ok(name) = device.name() {
                out.push(name);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn get_sound_inputs() -> Vec<String> {
    crate::platform::linux::get_pa_sources()
        .drain(..)
        .map(|x| x.1)
        .collect()
}

// sacrifice some memory
pub fn value_crash_workaround(values: &[Value]) -> Arc<Vec<Value>> {
    let persist = Arc::new(values.to_vec());
    STUPID_VALUES.lock().unwrap().push(persist.clone());
    persist
}

pub fn get_icon() -> String {
    // 128x128
    #[cfg(target_os = "macos")]
    // 128x128 on 160x160 canvas, then shrink to 128, mac looks better with padding
    {
        "data:image/png;base64,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".into()
    }
    #[cfg(not(target_os = "macos"))] // 128x128 no padding
    {
        "data:image/png;base64,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".into()
    }
}
