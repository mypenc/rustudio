//! Download Studio's built-in content from Roblox's CDN into the folders
//! `XDG_DATA_HOME` points at, the way rustudio does on a first launch. The
//! "Studio content" workflow (.github/workflows/studio-content.yml) runs this,
//! zips the result, and File > Import Studio Content… on a machine with no
//! internet unpacks it.

fn main() {
    if let Err(e) = rustudio_core::studio_content::install(&|s| eprintln!("studio content: {s}")) {
        eprintln!("studio content: {e}");
        std::process::exit(1);
    }
    if let Err(e) = rustudio_core::studio_content::fetch_classic_oof() {
        eprintln!("classic death sound: {e}");
        std::process::exit(1);
    }
    println!("{}", rustudio_core::studio_content::install_dir().display());
}
