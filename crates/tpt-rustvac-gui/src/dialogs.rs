//! Native dialogs. Destructive actions always go through a confirmation that lists the
//! exact absolute paths and the total size. Permanent delete needs a second, explicit step.

use std::path::{Path, PathBuf};

use fltk::dialog;
use fltk::menu::MenuItem;

use crate::model::{human, ActionKind, Item};

const LIST_LIMIT: usize = 25;

fn summary(items: &[Item]) -> String {
    let total: u64 = items.iter().map(|i| i.bytes).sum();
    let mut out = format!("Total: {}\n\n", human(total));
    for item in items.iter().take(LIST_LIMIT) {
        out.push_str(&format!(
            "{:>10}  {}\n",
            human(item.bytes),
            item.path.display()
        ));
    }
    if items.len() > LIST_LIMIT {
        out.push_str(&format!("…and {} more\n", items.len() - LIST_LIMIT));
    }
    out
}

pub fn pick_folder(current: &Path) -> Option<PathBuf> {
    dialog::dir_chooser("Choose a folder to scan", &current.to_string_lossy(), false)
        .map(PathBuf::from)
}

/// Asks before archiving. Returns true only on an explicit "Compress".
pub fn confirm_compress(items: &[Item]) -> bool {
    let msg = format!(
        "Compress {} item(s) to the vacuum, then move the originals to the trash?\n\n{}",
        items.len(),
        summary(items)
    );
    dialog::choice2_default(&msg, "Cancel", "Compress", "") == Some(1)
}

/// Asks before removing a folder without archiving. Move to Trash is the default.
/// Permanent delete needs a second confirmation.
pub fn confirm_remove(items: &[Item]) -> Option<ActionKind> {
    let msg = format!("Remove {} item(s)?\n\n{}", items.len(), summary(items));
    match dialog::choice2_default(&msg, "Cancel", "Move to Trash", "Delete permanently…") {
        Some(1) => Some(ActionKind::Trash),
        Some(2) => {
            let second = format!(
                "Permanently delete {} item(s)? This cannot be undone.\n\n{}",
                items.len(),
                summary(items)
            );
            (dialog::choice2_default(&second, "Cancel", "Yes, delete permanently", "") == Some(1))
                .then_some(ActionKind::DeletePermanently)
        }
        _ => None,
    }
}

/// Opens the system file manager with the path selected where supported.
pub fn reveal(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let parent = path.parent().unwrap_or(path);
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
}

/// Right-click menu for a row. Returns the label of the chosen item, if any.
/// "Compress" is only offered for targets that can be compressed.
pub fn row_menu(x: i32, y: i32, compressible: bool) -> Option<String> {
    let labels: Vec<&'static str> = if compressible {
        vec!["Compress", "Move to Trash", "Reveal", "Copy path"]
    } else {
        vec!["Move to Trash", "Reveal", "Copy path"]
    };
    let menu = MenuItem::new(&labels);
    menu.popup(x, y)
        .map(|item| item.label().unwrap_or_default())
}
