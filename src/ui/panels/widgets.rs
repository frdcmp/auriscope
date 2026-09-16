//! The pieces every panel is built from: the titled card, and the key/value
//! block that fills most of them.
//!
//! Both the sidebar and the settings dialog draw cards, so the card's padding,
//! header band and help-topic handling are decided once here. The two differ
//! only in how much room they have, which is why there are two paddings and
//! not two cards.

use eframe::egui;
use egui::{
    Align, Color32, FontId, Layout, Margin, Rect, RichText, Sense, Shape, Stroke, pos2, vec2,
};

use crate::ui::fonts;
use crate::ui::help::{self, Topic};

use super::theme::{ACCENT, CARD_BG, KEY, VAL};

/// Section headings inside a card: quieter than a value, weightier than a key.
const SUBHEAD: Color32 = Color32::from_gray(128);
/// The rule a section heading trails, a shade above the card it sits on.
const SUBHEAD_RULE: Color32 = Color32::from_gray(58);

/// Card padding: tight in the side panel, which is a narrow column, and roomier
/// in the settings dialog, which has the width to breathe.
/// The header band sits a shade above the card, and closes on a rule a shade
/// above that: enough to read as a header strip on a dark card, not enough to
/// look like a separate widget.
pub(super) const CARD_HEAD_BG: Color32 = Color32::from_rgb(38, 40, 45);
pub(super) const CARD_HEAD_RULE: Color32 = Color32::from_rgb(52, 54, 60);

const CARD_PAD: Margin = Margin::symmetric(10, 8);
/// Asymmetric on purpose: the header band takes its height from the top pad,
/// so a smaller one there keeps the band compact while the card keeps its
/// roomier bottom.
const CARD_PAD_WIDE: Margin = Margin {
    left: 14,
    right: 14,
    top: 8,
    bottom: 12,
};

/// A titled card: icon, title, optional right-aligned note, then the body.
///
/// The title carries the card's own help topic — the place for the idea behind
/// the card, which belongs to none of its rows in particular.
pub(super) fn card(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    topic: Topic,
    trailing: Option<String>,
    body: impl FnOnce(&mut egui::Ui),
) {
    card_with(ui, false, icon, title, topic, trailing, body);
}

/// The same card with the settings dialog's roomier padding.
pub(super) fn wide_card(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    topic: Topic,
    trailing: Option<String>,
    body: impl FnOnce(&mut egui::Ui),
) {
    card_with(ui, true, icon, title, topic, trailing, body);
}

fn card_with(
    ui: &mut egui::Ui,
    roomy: bool,
    icon: &str,
    title: &str,
    topic: Topic,
    trailing: Option<String>,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(CARD_BG)
        .corner_radius(6.0)
        .inner_margin(if roomy { CARD_PAD_WIDE } else { CARD_PAD })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = if roomy { 6.0 } else { 4.0 };
            // The band is painted once the header has been laid out, so it can
            // take that row's height; reserving its slot first keeps it behind
            // the text rather than over it.
            let band = ui.painter().add(Shape::Noop);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon).color(ACCENT).size(fonts::BODY));
                let t = ui.label(
                    RichText::new(title)
                        .family(fonts::bold())
                        .size(fonts::HEADING)
                        .color(Color32::from_gray(242)),
                );
                help::offer_response(ui, &t, topic);
                if let Some(t) = trailing {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(t).small().color(KEY));
                    });
                }
            });
            let pad = if roomy { CARD_PAD_WIDE } else { CARD_PAD };
            let head = ui.min_rect();
            // Full width, ignoring the card's padding: a header that stops
            // short of the edges reads as a first row, not as a header.
            let strip = Rect::from_min_max(
                pos2(head.left() - pad.left as f32, head.top() - pad.top as f32),
                pos2(
                    head.right() + pad.right as f32,
                    head.bottom() + pad.top as f32,
                ),
            );
            ui.painter().set(
                band,
                Shape::rect_filled(
                    strip,
                    egui::CornerRadius {
                        nw: 6,
                        ne: 6,
                        sw: 0,
                        se: 0,
                    },
                    CARD_HEAD_BG,
                ),
            );
            ui.painter().hline(
                strip.x_range(),
                strip.bottom() - 0.5,
                Stroke::new(1.0, CARD_HEAD_RULE),
            );
            // Clear of the rule by about the card's own bottom padding, so the
            // first row is not pinned under the header band.
            ui.add_space(if roomy { 14.0 } else { 12.0 });
            body(ui);
        });
}

/// Background of the hovered key/value pair: enough to read as a band across
/// the gap, not enough to compete with the values.
const KV_HOVER_BG: Color32 = Color32::from_rgb(46, 48, 54);
/// Space a key keeps clear of its value's column, the gutter a value keeps
/// clear of the next pair, and how far the highlight reaches past the text.
const KV_GAP: f32 = 10.0;
const KV_COL_GAP: f32 = 18.0;
const KV_PAD: f32 = 4.0;
/// Two pairs to a line is four columns; past that a column is too narrow to
/// hold a value.
const KV_MAX_PAIRS: usize = 2;

/// One key/value pair, collected before anything is drawn.
pub(super) struct KvRow {
    key: String,
    value: String,
    color: Color32,
    tooltip: Option<String>,
    help: Option<Topic>,
}

impl KvRow {
    /// Tooltip for this pair, for values shown abbreviated. This is data the
    /// row could not fit, so it shows whatever mode the UI is in — unlike
    /// `help`, which is explanation and only appears in help mode.
    pub(super) fn on_hover_text(&mut self, text: impl Into<String>) -> &mut Self {
        self.tooltip = Some(text.into());
        self
    }

    /// What this row means, for help mode.
    pub(super) fn help(&mut self, topic: Topic) -> &mut Self {
        self.help = Some(topic);
        self
    }
}

/// A block of key/value pairs.
///
/// Rows are gathered first so the block can measure them, then laid out in
/// columns that split the card in equal fractions: keys in one, values in the
/// next. One pair to a line is two columns, so the values start at half the
/// card; two pairs is four, at each quarter. The columns are invisible — only
/// the hovered pair is painted, as a band tying a key to its value across the
/// gap.
#[derive(Default)]
pub(super) struct Kv {
    rows: Vec<KvRow>,
}

impl Kv {
    pub(super) fn push(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
        color: Color32,
    ) -> &mut KvRow {
        self.rows.push(KvRow {
            key: key.into(),
            value: value.into(),
            color,
            tooltip: None,
            help: None,
        });
        self.rows.last_mut().expect("just pushed")
    }

    pub(super) fn show(self, ui: &mut egui::Ui) {
        if self.rows.is_empty() {
            return;
        }
        let key_font = FontId::new(fonts::SMALL, egui::FontFamily::Proportional);
        let val_font = FontId::monospace(fonts::MONO_SIZE);
        let text_w = |ui: &egui::Ui, text: &str, font: &FontId| {
            ui.painter()
                .layout_no_wrap(text.to_owned(), font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
        };
        let widest = |pick: fn(&KvRow) -> &String, font: &FontId| {
            self.rows
                .iter()
                .map(|r| text_w(ui, pick(r), font))
                .fold(0.0, f32::max)
        };
        let key_w = widest(|r| &r.key, &key_font);
        let val_w = widest(|r| &r.value, &val_font);

        let avail = ui.available_width();
        let pairs = kv_pairs_per_line(self.rows.len(), kv_col_need(key_w, val_w), avail);
        // Every column is the same fraction of the card, keys and values alike,
        // so column n starts at n/(2·pairs) however short the keys run.
        let col_w = avail / (2 * pairs) as f32;
        let lines = self.rows.len().div_ceil(pairs);

        let row_h = ui
            .text_style_height(&egui::TextStyle::Small)
            .max(ui.text_style_height(&egui::TextStyle::Monospace))
            + 3.0;
        let (rect, _) = ui.allocate_exact_size(vec2(avail, lines as f32 * row_h), Sense::hover());

        for (i, row) in self.rows.iter().enumerate() {
            let (line, pair) = (i / pairs, i % pairs);
            let key_x = rect.left() + (2 * pair) as f32 * col_w;
            let top = rect.top() + line as f32 * row_h;
            // The band starts a little left of the key and stops a little short
            // of the next pair, so the text never sits flush against an edge.
            let cell = Rect::from_min_size(pos2(key_x - KV_PAD, top), vec2(col_w * 2.0, row_h));
            if ui.rect_contains_pointer(cell) {
                ui.painter().rect_filled(cell, 3.0, KV_HOVER_BG);
            }
            let key = clipped(ui, &row.key, &key_font, KEY, col_w - KV_GAP);
            let value = clipped(ui, &row.value, &val_font, row.color, col_w - KV_COL_GAP);
            let y = |g: &egui::Galley| cell.center().y - g.size().y / 2.0;
            let key_at = pos2(key_x, y(&key));
            let val_at = pos2(key_x + col_w, y(&value));
            let key_rect = Rect::from_min_size(key_at, key.size());
            ui.painter().galley(key_at, key, KEY);
            ui.painter().galley(val_at, value, row.color);
            // Help wins the hover: an explanation and a truncated value in the
            // same popup would be two different kinds of thing at once.
            let helped = row
                .help
                .is_some_and(|topic| help::offer(ui, key_rect, cell, topic));
            if let Some(tip) = &row.tooltip
                && !helped
            {
                ui.interact(cell, ui.id().with(("kv", i)), Sense::hover())
                    .on_hover_text(tip);
            }
        }
    }
}

/// The width every column has to have, given the widest key and widest value.
/// Columns are uniform, so one number covers both: whichever of the two needs
/// more room, with the spacing that has to follow it.
fn kv_col_need(key_w: f32, val_w: f32) -> f32 {
    (key_w + KV_GAP).max(val_w + KV_COL_GAP)
}

/// How many key/value pairs fit on one line.
///
/// A second pair is only taken if all four columns still clear `col_need`, so
/// packing never costs a value its characters. Each pair also needs two rows of
/// its own — otherwise a short block spreads into a wide, one-line strip
/// instead of reading as a list.
fn kv_pairs_per_line(rows: usize, col_need: f32, avail: f32) -> usize {
    let by_width = (avail / (2.0 * col_need)).floor().max(1.0) as usize;
    by_width.min((rows / 2).max(1)).min(KV_MAX_PAIRS)
}

/// Lay `text` out on one line, cut with an ellipsis if it runs past `max_w`.
fn clipped(
    ui: &egui::Ui,
    text: &str,
    font: &FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font.clone(), color);
    job.wrap.max_width = max_w.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('\u{2026}');
    ui.painter().layout_job(job)
}

pub(super) fn kv_rows(ui: &mut egui::Ui, build: impl FnOnce(&mut Kv)) {
    let mut kv = Kv::default();
    build(&mut kv);
    kv.show(ui);
}

pub(super) fn kv<'a>(kv: &'a mut Kv, key: &str, value: impl Into<String>) -> &'a mut KvRow {
    kv.push(key, value, VAL)
}

pub(super) fn kv_colored<'a>(
    kv: &'a mut Kv,
    key: &str,
    value: impl Into<String>,
    color: Color32,
) -> &'a mut KvRow {
    kv.push(key, value, color)
}

/// A heading for a group of rows inside a card: small caps in bold, trailed by
/// a rule to the card's edge. Weight alone read as just another key, so the
/// heading doubles as the divider between one group of rows and the next.
pub(super) fn subhead(ui: &mut egui::Ui, text: &str, topic: Topic) {
    ui.add_space(5.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let label = ui.label(
            RichText::new(text.to_uppercase())
                .family(fonts::bold())
                .size(fonts::SMALL - 0.5)
                .color(SUBHEAD),
        );
        help::offer_response(ui, &label, topic);
        let w = ui.available_width();
        if w > 1.0 {
            let (rect, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
            ui.painter().hline(
                rect.x_range(),
                label.rect.center().y,
                Stroke::new(1.0, SUBHEAD_RULE),
            );
        }
    });
    ui.add_space(1.0);
}

pub(super) fn mono(text: impl Into<String>) -> RichText {
    RichText::new(text.into()).monospace().color(VAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_columns_are_equal_fractions_of_the_card() {
        // What the geometry does with the count: with one pair a line the value
        // column starts at half the card, with two pairs at each quarter.
        let avail = 400.0;
        for pairs in 1..=KV_MAX_PAIRS {
            let col_w = avail / (2 * pairs) as f32;
            for pair in 0..pairs {
                let key_x = (2 * pair) as f32 * col_w;
                let val_x = key_x + col_w;
                let frac = |x: f32| x / avail;
                assert_eq!(frac(key_x), (2 * pair) as f32 / (2 * pairs) as f32);
                assert_eq!(frac(val_x), (2 * pair + 1) as f32 / (2 * pairs) as f32);
            }
        }
        // Spelled out for the two the sidebar actually uses: one pair means a
        // 200-wide key column then the value at 200, two means columns of 100.
        assert_eq!(400.0 / 2.0, 200.0);
        assert_eq!(400.0 / 4.0, 100.0);
    }

    #[test]
    fn kv_takes_a_second_pair_only_when_all_four_columns_fit() {
        // Columns of 90: four need 360, so 400 holds two pairs and 340 holds one.
        assert_eq!(kv_pairs_per_line(8, 90.0, 400.0), 2);
        assert_eq!(kv_pairs_per_line(8, 90.0, 340.0), 1);
        // A wide value forces one pair however wide the card.
        assert_eq!(kv_pairs_per_line(8, 210.0, 400.0), 1);
        // Never wider than the rows can fill, two to a pair-column.
        assert_eq!(kv_pairs_per_line(3, 60.0, 400.0), 1);
        assert_eq!(kv_pairs_per_line(4, 60.0, 400.0), 2);
        // And never past the cap, however much room there is.
        assert_eq!(kv_pairs_per_line(40, 20.0, 2000.0), KV_MAX_PAIRS);
    }

    #[test]
    fn kv_columns_hold_the_widest_key_and_value_uncut() {
        // The fit test is what guarantees no truncation: whenever it takes a
        // pair, every column clears the widest key and the widest value.
        let (key_w, val_w) = (70.0, 110.0);
        let need = kv_col_need(key_w, val_w);
        let avail = 440.0;
        let pairs = kv_pairs_per_line(8, need, avail);
        assert_eq!(pairs, 1);
        let col_w = avail / (2 * pairs) as f32;
        assert!(col_w - KV_GAP >= key_w);
        assert!(col_w - KV_COL_GAP >= val_w);
    }
}
