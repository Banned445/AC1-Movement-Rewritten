//! Runtime loading of the user's own Assassin's Creed data (iw4L-style: nothing is redistributed).
//! Formats are documented in RE/08 (.forge) and RE/09 (Mesh / Skeleton / TextureMap).

pub mod ac_actions;
pub mod body_parts;
pub mod ac_anim;
pub mod ac_formats;
pub mod altair;
pub mod anims;
pub mod forge;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod probe;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The file that remembers the game folder (one line, the path), looked for in the working directory and next to
/// the executable. `--game-dir <path>` writes it.
pub const GAME_DIR_FILE: &str = "game_dir.txt";

static GAME_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// A folder holds the game when it has `DataPC.forge`.
pub fn is_game_dir(dir: &Path) -> bool {
    dir.join("DataPC.forge").is_file()
}

/// Use `dir` as the game folder from now on and remember it in `game_dir.txt` next to the executable.
pub fn set_game_dir(dir: &Path) -> Result<PathBuf, String> {
    if !is_game_dir(dir) {
        return Err(format!("{} has no DataPC.forge (pick the folder with AssassinsCreed_Dx9.exe in it)", dir.display()));
    }
    let file = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join(GAME_DIR_FILE))).unwrap_or_else(|| PathBuf::from(GAME_DIR_FILE));
    std::fs::write(&file, dir.display().to_string()).map_err(|e| format!("could not save {}: {e}", file.display()))?;
    let _ = GAME_DIR.set(Some(dir.to_path_buf()));
    Ok(file)
}

/// The Assassin's Creed install folder, or None when it can't be found. In order:
/// 1. `AC_GAME_DIR`;
/// 2. `game_dir.txt` in the working directory or next to the executable (written by `--game-dir`);
/// 3. a folder named `Assassin's Creed` in or beside any folder above the working directory or the executable;
/// 4. Steam: the default library and every library in `steamapps/libraryfolders.vdf`.
pub fn find_game_dir() -> Option<PathBuf> {
    GAME_DIR.get_or_init(search_game_dir).clone()
}

/// The game folder, or a placeholder path that fails to open (callers report the error).
pub fn game_dir() -> PathBuf {
    find_game_dir().unwrap_or_else(|| PathBuf::from("Assassin's Creed"))
}

fn search_game_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AC_GAME_DIR") {
        return Some(PathBuf::from(d));
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let starts: Vec<PathBuf> = [std::env::current_dir().ok(), exe_dir].into_iter().flatten().collect();
    for start in &starts {
        if let Ok(text) = std::fs::read_to_string(start.join(GAME_DIR_FILE)) {
            let dir = PathBuf::from(text.trim().trim_matches('"'));
            if is_game_dir(&dir) {
                return Some(dir);
            }
        }
    }
    for start in &starts {
        for dir in start.ancestors() {
            let candidate = dir.join("Assassin's Creed");
            if is_game_dir(&candidate) {
                return Some(candidate);
            }
        }
    }
    steam_libraries().into_iter().map(|lib| lib.join("steamapps").join("common").join("Assassin's Creed")).find(|d| is_game_dir(d))
}

/// Steam library folders: the default install locations plus the `"path"` entries of `libraryfolders.vdf`.
fn steam_libraries() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(|v| std::env::var_os(v))
        .map(|p| PathBuf::from(p).join("Steam"))
        .collect();
    roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    let mut libs = roots.clone();
    for root in &roots {
        let Ok(vdf) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) else { continue };
        for line in vdf.lines() {
            let parts: Vec<&str> = line.split('"').filter(|s| !s.trim().is_empty()).collect();
            if parts.len() == 2 && parts[0] == "path" {
                libs.push(PathBuf::from(parts[1].replace("\\\\", "\\")));
            }
        }
    }
    libs
}
