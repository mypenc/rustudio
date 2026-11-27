//! Roblox Studio's built-in content, downloaded on demand.
//!
//! rustudio ships no Roblox files. When this machine has no Studio install
//! (no `RUSTUDIO_STUDIO_DIR`, no `~/.local/share/rustudio/content` link),
//! rustudio downloads the parts of Studio it reads from Roblox's own
//! deployment CDN (`setup.rbxcdn.com`), the same packages the Studio
//! installer and Linux launchers such as Vinegar fetch, and unpacks them in
//! Studio's folder layout under `~/.local/share/rustudio/studio`:
//!
//! - `content/{avatar,fonts,sky,sounds,textures,models,configs}`: default
//!   characters, fonts, sky, sounds and `rbxasset://` textures,
//! - `PlatformContent/pc/{textures,terrain,fonts}`: studs, terrain colours,
//! - `ExtraContent/{scripts,models,textures}`: the default PlayerModule
//!   (camera + controls), character sounds and other built-in scripts,
//! - `ReflectionMetadata.xml`, taken alone out of `RobloxStudio.zip` with
//!   HTTP range requests (about 30 KB instead of the 120 MB package).
//!
//! About 60 MB to download. `rustudio --install-studio-content` does it in
//! the terminal; a normal launch does it in the background the first time
//! (`RUSTUDIO_NO_STUDIO_FETCH=1` turns that off).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// (package, folder it unpacks into, relative to the Studio root).
const PACKAGES: &[(&str, &str)] = &[
    ("content-avatar.zip", "content/avatar"),
    ("content-configs.zip", "content/configs"),
    ("content-fonts.zip", "content/fonts"),
    ("content-models.zip", "content/models"),
    ("content-sky.zip", "content/sky"),
    ("content-sounds.zip", "content/sounds"),
    ("content-textures2.zip", "content/textures"),
    ("content-textures3.zip", "PlatformContent/pc/textures"),
    ("content-terrain.zip", "PlatformContent/pc/terrain"),
    ("content-platform-fonts.zip", "PlatformContent/pc/fonts"),
    ("extracontent-scripts.zip", "ExtraContent/scripts"),
    ("extracontent-models.zip", "ExtraContent/models"),
    ("extracontent-textures.zip", "ExtraContent/textures"),
];
/// Single files pulled out of a big package by range requests.
const SINGLE_FILES: &[(&str, &str)] = &[("RobloxStudio.zip", "ReflectionMetadata.xml")];

const VERSION_URL: &str = "https://clientsettingscdn.roblox.com/v2/client-version/WindowsStudio64";
const CDN: &str = "https://setup.rbxcdn.com";
const MARKER: &str = ".rustudio-studio-version";
// Bump when the installed content layout changes, even within one CDN version.
const CONTENT_REVISION: &str = "legacy-textures-1";

/// Where the downloaded content lives.
pub fn install_dir() -> PathBuf {
    if let Ok(p) = std::env::var("RUSTUDIO_STUDIO_DOWNLOAD_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::paths::data().join("rustudio/studio")
}

/// The downloaded Studio root, when a complete download is there.
pub fn installed() -> Option<PathBuf> {
    let d = install_dir();
    d.join(MARKER).is_file().then_some(d)
}

static MESSAGES: Mutex<Vec<(bool, String)>> = Mutex::new(Vec::new());

/// Messages for Output from the background download: (is_error, text).
pub fn take_messages() -> Vec<(bool, String)> {
    MESSAGES.lock().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default()
}
fn post(err: bool, t: String) {
    if let Ok(mut m) = MESSAGES.lock() {
        m.push((err, t));
    }
}

/// On a normal launch with no Studio content anywhere, download it in the
/// background and report to Output.
pub fn spawn_background_install_if_missing() {
    fetch_classic_oof_once();
    if std::env::var_os("RUSTUDIO_NO_STUDIO_FETCH").is_some() {
        return;
    }
    if let Some(root) = crate::default_scripts::studio_dir() {
        // Upgrade older rustudio downloads without replacing the rest of their content.
        let need_vs = !has_view_selector(&root);
        let need_lp = !has_lua_package_assets(&root);
        let need_ui = !has_studio_ui_textures(&root);
        if root == install_dir() && (need_vs || need_lp || need_ui) {
            if let Some(marker) = std::fs::read_to_string(root.join(MARKER)).ok() {
                if let Some(version) = marker.split_whitespace().next().and_then(studio_version) {
                    let version = version.to_owned();
                    let _ = std::thread::Builder::new().name("studio-content-upgrade".into()).spawn(move || {
                        let a = agent();
                        if need_vs {
                            if let Err(e) = install_view_selector(&a, &version, &root) {
                                post(true, format!("Couldn't download Studio's View Selector: {e}"));
                            }
                        }
                        if need_lp {
                            if let Err(e) = install_lua_package_assets(&a, &version, &root) {
                                post(true, format!("Couldn't download Studio's LuaPackages assets: {e}"));
                            }
                        }
                        if need_ui {
                            if let Err(e) = install_studio_ui_textures(&a, &version, &root) {
                                post(true, format!("Couldn't download Studio's UI textures: {e}"));
                            }
                        }
                    });
                }
            }
        }
        return;
    }
    let _ = std::thread::Builder::new().name("studio-content".into()).spawn(|| {
        post(false, "No Roblox Studio install found: downloading Studio's built-in content (default character, player scripts, fonts, textures, sounds; about 60 MB) from Roblox's CDN…".into());
        match install(&|_| {}) {
            Ok(v) => post(false, format!(
                "Studio content {v} installed in {}. Play now uses Roblox's default character and player scripts; restart rustudio to use Studio's fonts everywhere.",
                install_dir().display()
            )),
            Err(e) => post(true, format!(
                "Couldn't download Studio's built-in content: {e}. Run `rustudio --install-studio-content` to retry, or point RUSTUDIO_STUDIO_DIR at a Studio install."
            )),
        }
    });
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .user_agent(concat!("rustudio/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str, range: Option<(u64, u64)>) -> Result<Vec<u8>, String> {
    let mut req = agent.get(url);
    if let Some((a, b)) = range {
        req = req.header("Range", &format!("bytes={a}-{b}"));
    }
    let mut resp = req.call().map_err(|e| format!("{url}: {e}"))?;
    if range.is_some() && resp.status().as_u16() != 206 {
        return Err(format!("{url}: no range support (HTTP {})", resp.status()));
    }
    resp.body_mut()
        .with_config()
        .limit(1 << 30)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))
}

fn content_length(agent: &ureq::Agent, url: &str) -> Result<u64, String> {
    let resp = agent.head(url).call().map_err(|e| format!("{url}: {e}"))?;
    resp.headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("{url}: no Content-Length"))
}

/// Download and unpack everything. Returns the Studio version installed.
/// `progress` gets one line per step.
/// Studio 0.533 (June 2022), the last build whose sounds package has the
/// classic death sound `uuhhh.mp3` ("Oof"); later Studios play a replacement.
const CLASSIC_OOF_PKG: &str = "version-58ed123c874e44c1-content-sounds.zip";

/// Where rustudio keeps the classic Oof it fetched from Roblox's CDN (its own
/// data folder: never inside a Studio install, never in the repo).
pub fn classic_oof() -> PathBuf {
    crate::paths::data().join("rustudio/classic-sounds/uuhhh.mp3")
}

/// Fetch the classic Oof in the background, once, unless it is already here.
pub fn fetch_classic_oof_once() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    // (RUSTUDIO_NO_STUDIO_FETCH or the rustudio option turns it off)
    if cfg!(test)
        || !crate::rustudio_settings::classic_death_sound_fetch()
        || classic_oof().is_file()
        || STARTED.swap(true, std::sync::atomic::Ordering::AcqRel)
    {
        return;
    }
    let _ = std::thread::Builder::new().name("classic-oof".into()).spawn(|| {
        if let Err(e) = fetch_classic_oof() {
            post(true, format!("Couldn't fetch the classic death sound (Studio plays its replacement meanwhile): {e}"));
        }
    });
}

/// Fetch the classic Oof now (also what `examples/export_studio_content.rs`
/// does for the offline zip).
pub fn fetch_classic_oof() -> Result<(), String> {
    let dest = classic_oof();
    fetch_one(&agent(), &format!("{CDN}/{CLASSIC_OOF_PKG}"), "uuhhh.mp3").and_then(|data| {
        let dir = dest.parent().ok_or("bad data dir")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = dir.join(".uuhhh.mp3.part");
        std::fs::write(&tmp, &data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())
    })
}

fn studio_version(version: &str) -> Option<&str> {
    version.strip_prefix("version-")
        .filter(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_alphanumeric()))
        .map(|_| version)
}

fn has_view_selector(root: &Path) -> bool {
    root.join("StudioContent/models/ViewSelector/Basic.mesh").is_file()
        && ["back", "bottom", "front", "left", "right", "top"].iter().all(|face| {
            ["", "_hover"].iter().all(|suffix| {
                root.join(format!("StudioContent/textures/ViewSelector/{face}{suffix}.png")).is_file()
            })
        })
}

fn install_view_selector(agent: &ureq::Agent, version: &str, root: &Path) -> Result<(), String> {
    // Publish the body last so the renderer can pick up the complete cube.
    for kind in ["textures", "models"] {
        let url = format!("{CDN}/{version}-studiocontent-{kind}.zip");
        let bytes = get(agent, &url, None)?;
        let zip = Zip::parse(&bytes).map_err(|e| format!("{url}: {e}"))?;
        for entry in &zip.entries {
            let Some((dir, file)) = entry.name.split_once(['\\', '/']) else { continue };
            if dir != "ViewSelector" || file.is_empty() || file.contains(['\\', '/']) {
                continue;
            }
            let data = zip.read(&bytes, entry).map_err(|e| format!("{url} {}: {e}", entry.name))?;
            write_file(&root.join("StudioContent").join(kind), &entry.name, &data)?;
        }
    }
    has_view_selector(root).then_some(()).ok_or_else(|| "Studio packages are missing View Selector files".into())
}

const LUA_PACKAGE_REQUIRED_FILES: &[&str] = &[
    "Packages/_Index/BuilderIcons/BuilderIcons/BuilderIcons.json",
    "Packages/_Index/BuilderIcons/BuilderIcons/Font/BuilderIcons-Regular.ttf",
    "Packages/_Index/BuilderIcons/BuilderIcons/Font/BuilderIcons-Filled.ttf",
    "Packages/_Index/FoundationImages/FoundationImages/SpriteSheets/img_set_1x_4.png",
];

fn is_lua_package_asset(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".png")
        || lower.ends_with(".ttf")
        || lower.ends_with(".otf")
        || lower.ends_with(".dds")
        || lower.ends_with(".webp")
        || lower.ends_with("buildericons.json")
}

fn has_lua_package_assets(root: &Path) -> bool {
    LUA_PACKAGE_REQUIRED_FILES
        .iter()
        .all(|rel| root.join("ExtraContent/LuaPackages").join(rel).is_file())
}

fn install_lua_package_assets(agent: &ureq::Agent, version: &str, root: &Path) -> Result<(), String> {
    let url = format!("{CDN}/{version}-extracontent-luapackages.zip");
    let bytes = get(agent, &url, None)?;
    let zip = Zip::parse(&bytes).map_err(|e| format!("{url}: {e}"))?;
    let base = root.join("ExtraContent/LuaPackages");
    for e in &zip.entries {
        if e.name.ends_with(['/', '\\']) || !is_lua_package_asset(&e.name) {
            continue;
        }
        let data = zip.read(&bytes, e).map_err(|err| format!("{url} {}: {err}", e.name))?;
        write_file(&base, &e.name, &data)?;
    }
    has_lua_package_assets(root)
        .then_some(())
        .ok_or_else(|| "Studio packages are missing LuaPackages UI assets".into())
}

fn has_studio_ui_textures(root: &Path) -> bool {
    root.join("content/studio_svg_textures/Shared/Utility/Dark/Standard/RadioOn.png").is_file()
        && root.join("StudioContent/textures/StudioToolbox/RoundedBackground.png").is_file()
}

fn install_studio_ui_textures(agent: &ureq::Agent, version: &str, root: &Path) -> Result<(), String> {
    for (pkg, sub) in [
        ("content-studio_svg_textures.zip", "content/studio_svg_textures"),
        ("studiocontent-textures.zip", "StudioContent/textures"),
    ] {
        let url = format!("{CDN}/{version}-{pkg}");
        let bytes = get(agent, &url, None)?;
        let zip = Zip::parse(&bytes).map_err(|e| format!("{url}: {e}"))?;
        let base = root.join(sub);
        for e in &zip.entries {
            if e.name.ends_with(['/', '\\']) {
                continue;
            }
            let data = zip.read(&bytes, e).map_err(|err| format!("{url} {}: {err}", e.name))?;
            write_file(&base, &e.name, &data)?;
        }
    }
    has_studio_ui_textures(root)
        .then_some(())
        .ok_or_else(|| "Studio packages are missing Studio UI textures".into())
}

pub fn install(progress: &dyn Fn(&str)) -> Result<String, String> {
    let agent = agent();
    let v: serde_json::Value = serde_json::from_slice(&get(&agent, VERSION_URL, None)?)
        .map_err(|e| format!("version answer: {e}"))?;
    let version = v["clientVersionUpload"]
        .as_str()
        .and_then(studio_version)
        .ok_or("version answer has no clientVersionUpload")?
        .to_string();
    progress(&format!("Studio {} ({version})", v["version"].as_str().unwrap_or("?")));

    let dest = install_dir();
    if std::fs::read_to_string(dest.join(MARKER)).is_ok_and(|s| s.trim() == format!("{version} {CONTENT_REVISION}")) {
        if !has_view_selector(&dest) {
            progress("downloading Studio View Selector");
            install_view_selector(&agent, &version, &dest)?;
        }
        if !has_lua_package_assets(&dest) {
            progress("downloading Studio LuaPackages UI assets");
            install_lua_package_assets(&agent, &version, &dest)?;
        }
        if !has_studio_ui_textures(&dest) {
            progress("downloading Studio UI textures");
            install_studio_ui_textures(&agent, &version, &dest)?;
        }
        progress("already installed");
        return Ok(version);
    }
    let parent = dest.parent().ok_or("bad install dir")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let partial = parent.join(format!(".studio-partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);

    let result = (|| -> Result<(), String> {
        for (pkg, sub) in PACKAGES {
            progress(&format!("downloading {pkg}"));
            let url = format!("{CDN}/{version}-{pkg}");
            let bytes = get(&agent, &url, None)?;
            let zip = Zip::parse(&bytes).map_err(|e| format!("{pkg}: {e}"))?;
            for e in &zip.entries {
                if e.name.ends_with(['/', '\\']) {
                    continue;
                }
                let data = zip.read(&bytes, e).map_err(|err| format!("{pkg} {}: {err}", e.name))?;
                write_file(&partial.join(sub), &e.name, &data)?;
            }
        }
        // Studio resolves retired rbxasset paths through LegacyContentTable,
        // not another deployment zip. Materialize its missing textures too so
        // a downloaded content tree is usable by file-based consumers.
        for line in include_str!("../../../assets/legacy-content.tsv").lines() {
            let Some((path, id)) = line.split_once('\t') else { continue };
            if !path.starts_with("textures/") || partial.join("content").join(path).is_file() {
                continue;
            }
            let id: u64 = id.trim().parse().map_err(|e| format!("legacy texture {path}: {e}"))?;
            progress(&format!("fetching legacy {path}"));
            match crate::assets::remote_cached(id) {
                Ok(bytes) => write_file(&partial.join("content"), path, &bytes)?,
                // Some retired entries have also been removed from asset
                // delivery. They must not prevent installing current content.
                Err(e) => progress(&format!("legacy {path} unavailable: {e}")),
            }
        }
        progress("downloading Studio View Selector");
        install_view_selector(&agent, &version, &partial)?;
        progress("downloading Studio LuaPackages UI assets");
        install_lua_package_assets(&agent, &version, &partial)?;
        progress("downloading Studio UI textures");
        install_studio_ui_textures(&agent, &version, &partial)?;
        for (pkg, file) in SINGLE_FILES {
            progress(&format!("fetching {file} from {pkg}"));
            let url = format!("{CDN}/{version}-{pkg}");
            let data = fetch_one(&agent, &url, file)?;
            write_file(&partial, file, &data)?;
        }
        std::fs::write(partial.join(MARKER), format!("{version} {CONTENT_REVISION}\n")).map_err(|e| e.to_string())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&partial, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    progress(&format!("installed in {}", dest.display()));
    Ok(version)
}

/// Import the zip the "Studio content" workflow makes
/// (`.github/workflows/studio-content.yml`), for a machine with no internet:
/// `studio/…` becomes the downloaded Studio content (`install_dir()`) and
/// `classic-sounds/…` the classic Oof's folder, exactly what `install` and
/// `fetch_classic_oof` leave. Returns the Studio version imported.
pub fn import_zip(path: &Path, progress: &dyn Fn(&str)) -> Result<String, String> {
    let classic_dir = classic_oof().parent().map(Path::to_path_buf).ok_or("bad data dir")?;
    import_zip_to(path, &install_dir(), &classic_dir, progress)
}

/// `import_zip` into the given folders; a zip that isn't complete or has a
/// path that would leave them changes nothing.
fn import_zip_to(path: &Path, dest: &Path, classic_dir: &Path, progress: &dyn Fn(&str)) -> Result<String, String> {
    progress(&format!("reading {}", path.display()));
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let zip = Zip::parse(&bytes).map_err(|e| format!("{}: not a zip ({e})", path.display()))?;
    let marker = format!("studio/{MARKER}");
    if !zip.entries.iter().any(|e| e.name.replace('\\', "/") == marker) {
        return Err(format!("{} isn't Studio content made by the rustudio workflow (no {marker} inside)", path.display()));
    }
    let parent = dest.parent().ok_or("bad install dir")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let partial = parent.join(format!(".studio-partial-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);

    let mut classic: Vec<(String, Vec<u8>)> = Vec::new();
    let result = (|| -> Result<(), String> {
        for e in &zip.entries {
            if e.name.ends_with(['/', '\\']) {
                continue;
            }
            let name = e.name.replace('\\', "/");
            if let Some(rest) = name.strip_prefix("studio/") {
                let data = zip.read(&bytes, e).map_err(|err| format!("{}: {err}", e.name))?;
                write_file(&partial, rest, &data)?;
            } else if let Some(rest) = name.strip_prefix("classic-sounds/") {
                let data = zip.read(&bytes, e).map_err(|err| format!("{}: {err}", e.name))?;
                classic.push((rest.to_string(), data));
            }
        }
        if partial.join(MARKER).is_file() {
            Ok(())
        } else {
            Err("the zip has no complete Studio content".into())
        }
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&partial, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    for (name, data) in classic {
        write_file(classic_dir, &name, &data)?;
    }
    let version = std::fs::read_to_string(dest.join(MARKER))
        .ok()
        .and_then(|m| m.split_whitespace().next().map(str::to_string))
        .unwrap_or_default();
    progress(&format!("imported into {}", dest.display()));
    Ok(version)
}

/// `import_zip` on a background thread, reporting to Output (the File menu's
/// Import Studio Content).
pub fn spawn_import(path: PathBuf) {
    let _ = std::thread::Builder::new().name("studio-content-import".into()).spawn(move || {
        post(false, format!("Importing Studio content from {}…", path.display()));
        match import_zip(&path, &|_| {}) {
            Ok(v) => post(false, format!(
                "Studio content {v} imported into {}. Restart rustudio to use Studio's fonts everywhere.",
                install_dir().display()
            )),
            Err(e) => post(true, format!("Couldn't import Studio content: {e}")),
        }
    });
}

/// Write `name` (a zip path, `\` or `/` separated) under `root`, refusing
/// anything that would land outside it.
fn write_file(root: &Path, name: &str, data: &[u8]) -> Result<(), String> {
    let mut path = root.to_path_buf();
    for part in name.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => return Err(format!("unsafe path in package: {name}")),
            p if p.contains(':') => return Err(format!("unsafe path in package: {name}")),
            p => path.push(p),
        }
    }
    if path == root {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, data).map_err(|e| format!("{}: {e}", path.display()))
}

/// One file out of a remote zip: the end record, the central directory and
/// that one entry, by range requests.
fn fetch_one(agent: &ureq::Agent, url: &str, file: &str) -> Result<Vec<u8>, String> {
    let mut out = fetch_many(agent, url, &[file])?;
    Ok(out.remove(0).1)
}

/// Several files out of a remote zip, reading the central directory once.
fn fetch_many<'a>(agent: &ureq::Agent, url: &str, files: &[&'a str]) -> Result<Vec<(&'a str, Vec<u8>)>, String> {
    let size = content_length(agent, url)?;
    let tail_len = size.min(65_536 + 22);
    let tail = get(agent, url, Some((size - tail_len, size - 1)))?;
    let (cd_off, cd_len) = Zip::end_record(&tail).map_err(|e| format!("{url}: {e}"))?;
    let cd = get(agent, url, Some((cd_off, cd_off + cd_len - 1)))?;
    let entries = Zip::central(&cd).map_err(|e| format!("{url}: {e}"))?;
    let mut out = Vec::with_capacity(files.len());
    for &file in files {
        let want = file.trim_start_matches(['\\', '/']).replace('\\', "/");
        let e = entries
            .iter()
            .find(|e| {
                e.name
                    .trim_start_matches(['\\', '/'])
                    .replace('\\', "/")
                    .eq_ignore_ascii_case(&want)
            })
            .ok_or_else(|| format!("{file} not in {url}"))?;
        // Local header (30 bytes + name + extra) then the data; read generously.
        let span = 30 + 1024 + e.comp_size;
        let chunk = get(agent, url, Some((e.offset, (e.offset + span).min(size) - 1)))?;
        let mut local = e.clone();
        local.offset = 0;
        out.push((file, Zip::read_entry(&chunk, &local)?));
    }
    Ok(out)
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    method: u16,
    crc: u32,
    comp_size: u64,
    size: u64,
    offset: u64,
}

/// A minimal zip reader (stored + deflate, no zip64): enough for Roblox's
/// deployment packages.
struct Zip {
    entries: Vec<Entry>,
}

fn u16_at(b: &[u8], i: usize) -> Result<u16, String> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or_else(|| "truncated zip".into())
}
fn u32_at(b: &[u8], i: usize) -> Result<u32, String> {
    b.get(i..i + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| "truncated zip".into())
}

impl Zip {
    fn parse(bytes: &[u8]) -> Result<Zip, String> {
        let (off, len) = Self::end_record(bytes)?;
        let cd = bytes
            .get(off as usize..(off + len) as usize)
            .ok_or("central directory out of range")?;
        Ok(Zip { entries: Self::central(cd)? })
    }

    /// (central directory offset, length) from the tail of a zip.
    fn end_record(tail: &[u8]) -> Result<(u64, u64), String> {
        let pos = tail
            .windows(4)
            .rposition(|w| w == b"PK\x05\x06")
            .ok_or("no zip end record")?;
        let len = u32_at(tail, pos + 12)? as u64;
        let off = u32_at(tail, pos + 16)? as u64;
        if len == 0xFFFF_FFFF || off == 0xFFFF_FFFF {
            return Err("zip64 not supported".into());
        }
        Ok((off, len))
    }

    fn central(cd: &[u8]) -> Result<Vec<Entry>, String> {
        let mut out = Vec::new();
        let mut p = 0;
        while p + 46 <= cd.len() && &cd[p..p + 4] == b"PK\x01\x02" {
            let method = u16_at(cd, p + 10)?;
            let crc = u32_at(cd, p + 16)?;
            let comp_size = u32_at(cd, p + 20)? as u64;
            let size = u32_at(cd, p + 24)? as u64;
            let nl = u16_at(cd, p + 28)? as usize;
            let el = u16_at(cd, p + 30)? as usize;
            let cl = u16_at(cd, p + 32)? as usize;
            let offset = u32_at(cd, p + 42)? as u64;
            let name = cd.get(p + 46..p + 46 + nl).ok_or("truncated zip")?;
            out.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                method,
                crc,
                comp_size,
                size,
                offset,
            });
            p += 46 + nl + el + cl;
        }
        Ok(out)
    }

    fn read(&self, bytes: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
        Self::read_entry(bytes, e)
    }

    /// The entry's data, given bytes where its local header starts at `e.offset`.
    fn read_entry(bytes: &[u8], e: &Entry) -> Result<Vec<u8>, String> {
        let h = e.offset as usize;
        if bytes.get(h..h + 4) != Some(b"PK\x03\x04") {
            return Err("bad local header".into());
        }
        let nl = u16_at(bytes, h + 26)? as usize;
        let el = u16_at(bytes, h + 28)? as usize;
        let start = h + 30 + nl + el;
        let comp = bytes
            .get(start..start + e.comp_size as usize)
            .ok_or("entry data out of range")?;
        let data = match e.method {
            0 => comp.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(e.size as usize);
                flate2::read::DeflateDecoder::new(comp)
                    .read_to_end(&mut out)
                    .map_err(|err| format!("inflate: {err}"))?;
                out
            }
            m => return Err(format!("compression method {m} not supported")),
        };
        let mut crc = flate2::Crc::new();
        crc.update(&data);
        if data.len() as u64 != e.size || crc.sum() != e.crc {
            return Err("checksum mismatch".into());
        }
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip of stored (uncompressed) files, as `zip -0` would make.
    fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut central) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let mut crc = flate2::Crc::new();
            crc.update(data);
            let offset = out.len() as u32;
            out.extend(b"PK\x03\x04");
            out.extend([20u8, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // version, flags, method 0, time, date
            for v in [crc.sum(), data.len() as u32, data.len() as u32] {
                out.extend(v.to_le_bytes());
            }
            out.extend((name.len() as u16).to_le_bytes());
            out.extend([0u8, 0]);
            out.extend(name.as_bytes());
            out.extend(*data);
            central.extend(b"PK\x01\x02");
            central.extend([20u8, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            for v in [crc.sum(), data.len() as u32, data.len() as u32] {
                central.extend(v.to_le_bytes());
            }
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0u8; 12]); // extra, comment, disk, internal attrs, external attrs
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend(&central);
        out.extend(b"PK\x05\x06");
        out.extend([0u8; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(cd_offset.to_le_bytes());
        out.extend([0u8, 0]);
        out
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rs-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The offline zip lands where `install` and `fetch_classic_oof` put things.
    #[test]
    fn imports_the_workflow_zip() {
        let d = temp("ok");
        let zip = d.join("studio-content.zip");
        std::fs::write(
            &zip,
            stored_zip(&[
                ("studio/content/fonts/a.ttf", b"font"),
                ("studio/.rustudio-studio-version", b"version-abc legacy-textures-1\n"),
                ("classic-sounds/uuhhh.mp3", b"oof"),
            ]),
        )
        .unwrap();
        let (dest, classic) = (d.join("data/studio"), d.join("data/classic-sounds"));
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old.txt"), "stale").unwrap();
        assert_eq!(import_zip_to(&zip, &dest, &classic, &|_| {}).unwrap(), "version-abc");
        assert_eq!(std::fs::read(dest.join("content/fonts/a.ttf")).unwrap(), b"font");
        assert!(!dest.join("old.txt").exists(), "the previous content is replaced");
        assert_eq!(std::fs::read(classic.join("uuhhh.mp3")).unwrap(), b"oof");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Not Studio content, or a path out of the folder: nothing changes.
    #[test]
    fn refuses_a_bad_zip_and_changes_nothing() {
        let d = temp("bad");
        let (dest, classic) = (d.join("data/studio"), d.join("data/classic-sounds"));
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("keep.txt"), "kept").unwrap();
        for (name, files) in [
            ("no-marker.zip", stored_zip(&[("studio/content/a.txt", b"x")])),
            (
                "escape.zip",
                stored_zip(&[("studio/.rustudio-studio-version", b"v r\n"), ("studio/../evil.txt", b"x")]),
            ),
            ("not-a-zip.zip", b"hello".to_vec()),
        ] {
            let zip = d.join(name);
            std::fs::write(&zip, files).unwrap();
            assert!(import_zip_to(&zip, &dest, &classic, &|_| {}).is_err(), "{name}");
            assert_eq!(std::fs::read_to_string(dest.join("keep.txt")).unwrap(), "kept", "{name}");
        }
        assert!(!d.join("data/evil.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_escaping_paths() {
        let d = std::env::temp_dir().join(format!("rs-sc-test-{}", std::process::id()));
        assert!(write_file(&d, "..\\evil.txt", b"x").is_err());
        assert!(write_file(&d, "a/../../evil.txt", b"x").is_err());
        assert!(write_file(&d, "C:\\evil.txt", b"x").is_err());
        assert!(write_file(&d, "\\sub\\ok.txt", b"x").is_ok());
        assert!(d.join("sub/ok.txt").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }
}
