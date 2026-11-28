//! `prefetch_packs ID…`: fetch each asset into rustudio's asset cache the way
//! the app does (public asset delivery, no login), and for a texture pack its
//! colour / normal / roughness / metalness / height / emissive maps too. The
//! "Studio content" workflow runs this for the part and terrain material
//! packs, so a machine with no internet has them in its cache.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

fn main() {
    let ids: VecDeque<u64> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).filter(|&i| i != 0).collect();
    let total = ids.len();
    let queue = Arc::new(Mutex::new(ids));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));
    let fetched = Arc::new(Mutex::new(0usize));
    let workers: Vec<_> = (0..6)
        .map(|_| {
            let (queue, failures, fetched) = (queue.clone(), failures.clone(), fetched.clone());
            std::thread::spawn(move || loop {
                let Some(id) = queue.lock().unwrap().pop_front() else { return };
                let mut children = Vec::new();
                match rustudio_core::assets::public_cached(id) {
                    Ok(bytes) => {
                        *fetched.lock().unwrap() += 1;
                        if let Ok(p) = rustudio_core::texture_pack::parse_pack(&String::from_utf8_lossy(&bytes)) {
                            children.extend([p.color, p.normal, p.roughness, p.metalness, p.height, p.emissive].into_iter().flatten());
                        }
                    }
                    Err(e) => failures.lock().unwrap().push(format!("{id}: {e}")),
                }
                for c in children {
                    match rustudio_core::assets::public_cached(c) {
                        Ok(_) => *fetched.lock().unwrap() += 1,
                        Err(e) => failures.lock().unwrap().push(format!("{id} map {c}: {e}")),
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    let failures = failures.lock().unwrap();
    for f in failures.iter() {
        eprintln!("failed: {f}");
    }
    println!("{total} ids asked, {} assets cached, {} failed", fetched.lock().unwrap(), failures.len());
    // A few retired assets may be gone; most of them missing is a real failure.
    if failures.len() * 10 > total.max(1) * 4 {
        std::process::exit(1);
    }
}
