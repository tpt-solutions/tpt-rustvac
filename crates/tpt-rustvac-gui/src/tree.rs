//! Painting and hit-testing for the tree. Each row is one line of the flattened tree:
//! indent · expander · checkbox · name … size · bar · age, with the metadata columns
//! anchored to the right edge so they line up for every row.

use std::time::SystemTime;

use fltk::draw;
use fltk::enums::{Align, Color, Font};

use crate::model::{human, is_rust_target, Model, Row};
use tpt_rustvac_core::model::State;

pub const ROW_H: i32 = 22;
const INDENT: i32 = 18;
const EXPANDER_W: i32 = 14;
const BOX_W: i32 = 14;
const SIZE_W: i32 = 92;
const BAR_W: i32 = 110;
const AGE_W: i32 = 72;
const GAP: i32 = 12;

const AMBER: (u8, u8, u8) = (230, 160, 0);
const RED: (u8, u8, u8) = (220, 60, 60);

/// What a click in a row landed on.
pub enum Hit {
    Expander,
    Checkbox,
    Name,
}

fn expander_x(depth: usize, x: i32) -> i32 {
    x + 6 + depth as i32 * INDENT
}

fn checkbox_x(depth: usize, x: i32) -> i32 {
    expander_x(depth, x) + EXPANDER_W + 4
}

/// Works out which part of a row a click falls on. `event_x` is in the same
/// client-area coordinates as the table, and `table_x` is the table's left edge.
pub fn hit_test(model: &Model, row: &Row, table_x: i32, event_x: i32) -> Hit {
    let base = table_x;
    let exp = expander_x(row.depth, base);
    let cb = checkbox_x(row.depth, base);
    if row.has_children && event_x >= exp && event_x < exp + EXPANDER_W {
        return Hit::Expander;
    }
    let is_target = model.arena.get(row.id).map(is_rust_target).unwrap_or(false);
    if is_target && event_x >= cb && event_x < cb + BOX_W {
        return Hit::Checkbox;
    }
    Hit::Name
}

/// Paints one row. `x, y, w, h` are the row's rectangle in window coordinates.
pub fn paint_row(model: &Model, row: &Row, x: i32, y: i32, w: i32, h: i32) {
    let Some(node) = model.arena.get(row.id) else {
        return;
    };
    let selected = model.selected == Some(row.id);
    let now = SystemTime::now();

    // Background.
    if selected {
        draw::set_draw_color(Color::from_rgb(56, 110, 188));
    } else {
        draw::set_draw_color(Color::from_rgb(255, 255, 255));
    }
    draw::draw_rectf(x, y, w, h);
    let text = if selected {
        Color::White
    } else {
        Color::from_rgb(30, 30, 30)
    };

    draw::set_font(Font::Helvetica, 13);

    // Expander: a plus or minus in a small box.
    if row.has_children {
        let ex = expander_x(row.depth, x);
        let ey = y + (h - EXPANDER_W) / 2;
        draw::set_draw_color(Color::from_rgb(120, 120, 120));
        draw::draw_rect(ex, ey, EXPANDER_W, EXPANDER_W);
        draw::set_draw_color(text);
        let mid = ey + EXPANDER_W / 2;
        draw::draw_line(ex + 3, mid, ex + EXPANDER_W - 4, mid);
        if !row.expanded {
            draw::draw_line(
                ex + EXPANDER_W / 2,
                ey + 3,
                ex + EXPANDER_W / 2,
                ey + EXPANDER_W - 4,
            );
        }
    }

    // Checkbox for rust targets.
    let is_target = is_rust_target(node);
    if is_target {
        let bx = checkbox_x(row.depth, x);
        let by = y + (h - BOX_W) / 2;
        draw::set_draw_color(Color::from_rgb(120, 120, 120));
        draw::draw_rect(bx, by, BOX_W, BOX_W);
        if model.checked.contains(&row.id) {
            draw::set_draw_color(if selected {
                Color::White
            } else {
                Color::from_rgb(56, 110, 188)
            });
            draw::draw_rectf(bx + 3, by + 3, BOX_W - 6, BOX_W - 6);
        }
    }

    // Right-hand columns, anchored to the right edge.
    let right = x + w - 8;
    let age_x = right - AGE_W;
    let bar_x = age_x - GAP - BAR_W;
    let size_x = bar_x - GAP - SIZE_W;

    // Age.
    let (age_text, age_color) = age_label(node.newest, now, selected);
    draw::set_draw_color(age_color);
    draw::draw_text2(&age_text, age_x, y, AGE_W, h, Align::Right | Align::Inside);

    // Proportional bar, scaled to the root total.
    let root_size = model.arena.root().map(|r| r.size).unwrap_or(0).max(1);
    let share = (node.size as f64 / root_size as f64).clamp(0.0, 1.0);
    let bar_y = y + h / 2 - 4;
    draw::set_draw_color(Color::from_rgb(225, 225, 225));
    draw::draw_rectf(bar_x, bar_y, BAR_W, 8);
    draw::set_draw_color(if selected {
        Color::White
    } else {
        Color::from_rgb(56, 110, 188)
    });
    draw::draw_rectf(bar_x, bar_y, (BAR_W as f64 * share) as i32, 8);

    // Size, monospace-looking and right-aligned.
    draw::set_font(Font::Courier, 13);
    draw::set_draw_color(text);
    draw::draw_text2(
        &human(node.size),
        size_x,
        y,
        SIZE_W,
        h,
        Align::Right | Align::Inside,
    );
    draw::set_font(Font::Helvetica, 13);

    // Name, then badges. Clipped so it never runs into the size column.
    let name_x = checkbox_x(row.depth, x) + if is_target { BOX_W + 8 } else { 0 };
    let name_w = (size_x - GAP - name_x).max(0);
    let mut label = node.name.clone();
    if is_target {
        label.push_str("   [rust]");
    }
    if node.state == State::Archived {
        label.push_str("   [archived]");
    }
    if is_kept_marker(model, row.id) {
        label.push_str("   [kept]");
    }
    match &node.state {
        State::Busy => label.push_str("   [busy]"),
        State::Failed(_) => label.push_str("   [failed]"),
        _ => {}
    }

    draw::push_clip(name_x, y, name_w, h);
    draw::set_draw_color(text);
    draw::draw_text2(&label, name_x, y, name_w, h, Align::Left | Align::Inside);
    if node.state == State::Archived {
        // Strike through the name, and only the name.
        let name_px = draw::width(&node.name) as i32;
        let sy = y + h / 2;
        draw::draw_line(name_x, sy, name_x + name_px, sy);
    }
    draw::pop_clip();
}

fn is_kept_marker(model: &Model, id: tpt_rustvac_core::model::NodeId) -> bool {
    let keep = model.config.keep_paths();
    model
        .arena
        .get(id)
        .map(|n| tpt_rustvac_core::safety::is_kept(&n.path, &keep))
        .unwrap_or(false)
}

fn age_label(newest: Option<SystemTime>, now: SystemTime, selected: bool) -> (String, Color) {
    let Some(age) = newest.and_then(|t| now.duration_since(t).ok()) else {
        return (
            "unknown".into(),
            if selected {
                Color::White
            } else {
                Color::from_rgb(130, 130, 130)
            },
        );
    };
    let days = age.as_secs() / 86_400;
    let text = format!("{days}d");
    let color = if selected {
        Color::White
    } else if days > 90 {
        Color::from_rgb(RED.0, RED.1, RED.2)
    } else if days > 30 {
        Color::from_rgb(AMBER.0, AMBER.1, AMBER.2)
    } else if days < 7 {
        Color::from_rgb(130, 130, 130)
    } else {
        Color::from_rgb(30, 30, 30)
    };
    (text, color)
}
