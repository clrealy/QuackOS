use duckfs::{mount::mount, DuckFs};
use std::path::PathBuf;
use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "duckfs - the filesystem for QuackOS 🦆

usage:
  duckfs mkfs  <image> <size-in-MiB>     create a new DuckFS image
  duckfs mount <image> <mountpoint>      mount it (needs FUSE; unmount with fusermount -u)
  duckfs info  <image>                   show geometry and free space"
    );
    exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["mkfs", image, mb] => {
            let mb: u64 = mb.parse().unwrap_or_else(|_| usage());
            DuckFs::format(&PathBuf::from(image), mb * 1024 * 1024).map(|_| println!("formatted {image} ({mb} MiB)"))
        }
        ["mount", image, mp] => mount(&PathBuf::from(image), &PathBuf::from(mp)),
        ["info", image] => DuckFs::open(&PathBuf::from(image)).and_then(|fs| {
            let s = fs.statfs()?;
            println!("DuckFS  block size {}  blocks {} ({} free)  inodes {} ({} free)", s.bsize, s.blocks, s.bfree, s.files, s.ffree);
            Ok(())
        }),
        _ => usage(),
    };
    if let Err(e) = res {
        eprintln!("duckfs: {e}");
        exit(1);
    }
}
