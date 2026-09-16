//! Help mode: a toggle in the transport bar that turns every labelled reading
//! and control into something you can hover for an explanation.
//!
//! Off by default and inert — nothing in the UI behaves differently until the
//! toggle is on, at which point hovering a label that carries a [`Topic`] pops
//! a card explaining it. The copy all lives in this file, in [`Topic::text`],
//! so it can be read and revised as prose rather than hunted through the
//! drawing code. The sidebar and the settings dialog describe the same
//! concepts in places, and a shared topic means that text is written once.
//!
//! Attaching a topic is one call at the point the label is drawn:
//! `kv(rows, "Sample rate", …).help(Topic::SampleRate)` in the sidebar,
//! `setting(ui, "Overlap", Some(Topic::Overlap), …)` in the dialog.

use eframe::egui;
use egui::{Color32, Id, Rect, RectAlign, RichText, Stroke, Ui};

use super::fonts;
use super::panels::{ACCENT, CARD_BG, VAL};

/// Width of a popover. Wide enough for a sentence to breathe, narrow enough
/// that it never blankets the panel it is explaining.
const POPOVER_W: f32 = 290.0;
/// How far the popover sits off the label it belongs to.
const POPOVER_GAP: f32 = 8.0;
/// Preferred side, then where to fall back when that side has no room: help
/// in the sidebar wants to open leftwards, help in the settings dialog
/// rightwards, and either flips rather than running off the window.
const PLACEMENT: RectAlign = RectAlign::LEFT_START;
const PLACEMENT_ALTS: [RectAlign; 4] = [
    RectAlign::RIGHT_START,
    RectAlign::LEFT_END,
    RectAlign::RIGHT_END,
    RectAlign::BOTTOM_START,
];

/// Whether help mode is on.
///
/// Kept in egui's per-frame data rather than passed down, because it is a
/// global mode and threading a `bool` through every drawing helper would put
/// it in a dozen signatures that have no other use for it. `App::ui` writes it
/// once a frame, before any panel is drawn.
pub fn set_enabled(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(id(), on));
}

pub fn enabled(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(id()).unwrap_or(false))
}

fn id() -> Id {
    Id::new("help-mode")
}

/// Offer help for `topic`.
///
/// `mark` is the label the dotted rule goes under; `hit` is the region that
/// has to be hovered, usually the whole row so either half of a key/value pair
/// works. Does nothing at all while help mode is off. Returns whether the
/// popover is showing.
pub fn offer(ui: &Ui, mark: Rect, hit: Rect, topic: Topic) -> bool {
    if !enabled(ui.ctx()) {
        return false;
    }
    underline(ui, mark);
    if !ui.rect_contains_pointer(hit) {
        return false;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
    popover(ui, hit, topic);
    true
}

/// [`offer`] for a widget that already has a response, which is how the
/// settings dialog's labels come.
pub fn offer_response(ui: &Ui, resp: &egui::Response, topic: Topic) -> bool {
    offer(ui, resp.rect, resp.rect, topic)
}

/// The "there is a definition here" mark: a dotted rule under the label, the
/// convention borrowed from print glossaries and `<abbr>` on the web.
fn underline(ui: &Ui, mark: Rect) {
    let y = mark.bottom() - 1.0;
    let painter = ui.painter();
    let mut x = mark.left();
    while x < mark.right() - 1.0 {
        painter.line_segment(
            [egui::pos2(x, y), egui::pos2((x + 1.5).min(mark.right()), y)],
            Stroke::new(1.0, ACCENT.gamma_multiply(0.55)),
        );
        x += 4.0;
    }
}

fn popover(ui: &Ui, anchor: Rect, topic: Topic) {
    let (title, body) = topic.text();
    let frame = egui::Frame::new()
        .fill(CARD_BG)
        .stroke(Stroke::new(1.0, Color32::from_gray(64)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .shadow(egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(120),
        });
    egui::Popup::new(
        Id::new(("help-popover", topic)),
        ui.ctx().clone(),
        anchor,
        ui.layer_id(),
    )
    .kind(egui::PopupKind::Tooltip)
    .align(PLACEMENT)
    .align_alternatives(&PLACEMENT_ALTS)
    .gap(POPOVER_GAP)
    .width(POPOVER_W)
    .frame(frame)
    .interactable(false)
    .show(|ui| {
        ui.label(
            RichText::new(title)
                .family(fonts::bold())
                .size(fonts::BODY)
                .color(ACCENT),
        );
        ui.add_space(3.0);
        ui.label(RichText::new(body).size(fonts::SMALL).color(VAL));
    });
}

/// The `?` toggle in the transport bar, drawn as vectors so no fallback font
/// decides how it looks — the same approach as the settings button beside it.
pub fn toggle_button(ui: &mut Ui, active: bool, size: egui::Vec2) -> egui::Response {
    let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
    let visuals = ui.style().visuals.clone();
    let fg = if active {
        visuals.widgets.active.fg_stroke.color
    } else if resp.hovered() {
        Color32::WHITE
    } else {
        Color32::from_gray(215)
    };
    if active || resp.hovered() {
        let bg = if active {
            visuals.widgets.active.weak_bg_fill
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        painter.rect_filled(resp.rect, 4.0, bg);
    }
    let c = resp.rect.center();
    painter.circle_stroke(c, 7.5, Stroke::new(1.4, fg));
    painter.text(
        c,
        egui::Align2::CENTER_CENTER,
        "?",
        egui::FontId::new(11.0, fonts::bold()),
        fg,
    );
    resp
}

/// Everything help mode can explain.
///
/// One variant per idea rather than per label: where the sidebar and the
/// settings dialog show the same thing — reassignment, the dB floor, the STFT
/// resolution — they point at the same topic, and the wording cannot drift
/// apart between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    // ---- cards ---------------------------------------------------------
    FileCard,
    WaveHeaderCard,
    BwfCard,
    TagsCard,
    MarkersCard,
    LoudnessCard,
    LevelsCard,
    AnalysisCard,
    CursorCard,

    // ---- file ----------------------------------------------------------
    Container,
    Codec,
    SampleRate,
    Channels,
    BitDepth,
    Duration,
    Frames,
    FileSize,
    BitRate,
    InMemory,
    Modified,

    // ---- WAVE header ---------------------------------------------------
    WavFormat,
    BlockAlign,
    ByteRate,
    ValidBits,
    ChannelMask,
    RiffBody,
    Chunks,

    // ---- broadcast wave ------------------------------------------------
    BwfOriginator,
    BwfReference,
    BwfOriginated,
    BwfTimeRef,
    BwfUmid,
    BwfStoredLoudness,

    // ---- loudness ------------------------------------------------------
    Integrated,
    LoudnessRange,
    MaxMomentary,
    MaxShortTerm,
    Correlation,
    AgainstTargets,
    TargetDelta,
    Headroom,
    CrestFactor,

    // ---- levels --------------------------------------------------------
    PeakLevel,
    TruePeakLevel,
    RmsLevel,
    DcOffset,
    Clipping,

    // ---- analysis ------------------------------------------------------
    AnWindow,
    AnHop,
    Resolution,
    Reassignment,
    DetailTile,
    ViewRange,
    ViewSpan,
    FloorCeiling,

    // ---- cursor --------------------------------------------------------
    Playhead,
    PointerReadout,
    Selection,
    RulerRange,
    LoopToggle,

    // ---- settings: cards -----------------------------------------------
    PanesCard,
    SpectrogramCard,
    WaveformCard,
    SpectrumCard,
    FilesCard,
    KeysCard,
    AboutCard,

    // ---- settings: panes -----------------------------------------------
    ShowPanes,
    MergeViews,
    MergeWaveOpacity,
    MergeSpecOpacity,

    // ---- settings: spectrogram -----------------------------------------
    WindowSize,
    Overlap,
    WindowKind,
    ColourMap,
    CustomColours,
    Contrast,
    FrequencyAxis,
    Floor,
    Ceiling,
    LowestFrequency,

    // ---- settings: waveform --------------------------------------------
    WaveOverlays,
    WaveColour,
    VerticalZoom,
    WaveHeight,

    // ---- settings: spectrum --------------------------------------------
    SpectrumSize,
    SpectrumAveraging,

    // ---- settings: files -----------------------------------------------
    RememberFiles,
}

impl Topic {
    /// Heading and body, together, so the copy reads as a document.
    fn text(self) -> (&'static str, &'static str) {
        use Topic as T;
        match self {
            // ---- cards -------------------------------------------------
            T::FileCard => (
                "File",
                "What the decoder found: the container it opened, the codec inside, and the \
                 shape of the audio once decoded. Everything below is read from the file, not \
                 measured from the sound. Click the folder line under the name to show the file \
                 in your file manager.",
            ),
            T::WaveHeaderCard => (
                "WAVE header",
                "The RIFF structure of a .wav, chunk by chunk. Useful when a file will not open \
                 elsewhere: a missing fmt chunk, an odd format tag or a truncated data chunk \
                 shows up here.",
            ),
            T::BwfCard => (
                "Broadcast Wave",
                "The bext chunk, added by broadcast and field recorders. It carries the \
                 timestamp, the originating machine and, often, loudness the recorder measured \
                 at the time.",
            ),
            T::TagsCard => (
                "Tags",
                "Free-text metadata: RIFF INFO fields in a .wav, or the codec's own tags \
                 elsewhere. Purely descriptive — nothing here affects playback.",
            ),
            T::MarkersCard => (
                "Markers",
                "Cue points stored in the file, each with a position and an optional label. \
                 Recorders drop them on a take; editors use them as sync points.",
            ),
            T::LoudnessCard => (
                "Loudness",
                "How loud the file sounds to a listener, measured to EBU R128 / ITU-R BS.1770. \
                 This is perceptual: it weights frequencies the way hearing does and averages \
                 over time, so it tracks impression rather than peak voltage.",
            ),
            T::LevelsCard => (
                "Levels",
                "Per-channel amplitude, in dBFS — decibels relative to full scale, where 0 is \
                 the loudest a sample can encode and everything else is negative.",
            ),
            T::AnalysisCard => (
                "Analysis",
                "The settings the spectrogram on screen was computed with, and what they work \
                 out to for this file's sample rate. Change them in Settings.",
            ),
            T::CursorCard => (
                "Cursor",
                "Where things are right now: the playhead, whatever the pointer is over, and \
                 any selection or range you have made.",
            ),

            // ---- file --------------------------------------------------
            T::Container => (
                "Container",
                "The file format wrapping the audio — WAV, FLAC, MP4 and so on. The container \
                 decides how the stream is packaged and what metadata can ride along; the codec \
                 decides how the audio itself is encoded.",
            ),
            T::Codec => (
                "Codec",
                "How the audio is encoded inside the container. PCM is uncompressed samples; \
                 FLAC and ALAC compress without loss; MP3 and AAC discard detail to save space. \
                 Hover the value for the decoder's full name for it.",
            ),
            T::SampleRate => (
                "Sample rate",
                "How many times a second the waveform was measured. It sets the highest \
                 frequency the file can represent — half the rate, the Nyquist limit — so 48 kHz \
                 audio carries up to 24 kHz and nothing above.",
            ),
            T::Channels => (
                "Channels",
                "How many separate signals the file holds: one for mono, two for a stereo pair, \
                 more for surround. Each gets its own waveform, spectrogram and level meters.",
            ),
            T::BitDepth => (
                "Bit depth",
                "How finely each sample is quantised. More bits means a lower noise floor and \
                 more headroom to work in: roughly 6 dB of dynamic range per bit, so 16-bit \
                 reaches about 96 dB and 24-bit about 144 dB.",
            ),
            T::Duration => (
                "Duration",
                "Length of the audio, as minutes and seconds. It is the frame count divided by \
                 the sample rate, so it is exact rather than rounded to the container's own \
                 estimate.",
            ),
            T::Frames => (
                "Frames",
                "How many sample points the file holds per channel. A frame is one instant \
                 across all channels, so a stereo file has two samples per frame.",
            ),
            T::FileSize => (
                "Size",
                "The file on disk, headers and metadata included. Compare it with the body of \
                 the data chunk to see how much of the file is not audio.",
            ),
            T::BitRate => (
                "Bit rate",
                "How many bits a second of audio takes. For PCM it falls straight out of rate × \
                 depth × channels; for a compressed codec it is what the encoder was asked to \
                 spend, and the usual proxy for quality.",
            ),
            T::InMemory => (
                "In memory",
                "What the decoded audio costs in RAM here: every sample expanded to 32-bit \
                 float, whatever it was on disk. A compressed file can be many times larger \
                 decoded than it is stored.",
            ),
            T::Modified => (
                "Modified",
                "The filesystem's last-write time, in UTC. This is the file, not the recording \
                 — a copy or a re-tag moves it. For when the audio was captured, look at the \
                 Broadcast Wave origination date.",
            ),

            // ---- WAVE header -------------------------------------------
            T::WavFormat => (
                "Format",
                "The fmt chunk's format tag. 0x0001 is integer PCM, 0x0003 float PCM, 0xFFFE \
                 the extensible form that carries a channel mask and a real sub-format. An \
                 unexpected tag here is why some players refuse a file.",
            ),
            T::BlockAlign => (
                "Block align",
                "Bytes per frame: channels × bit depth ÷ 8. Every read of the data chunk has to \
                 land on a multiple of it, so it is the granularity of any seek.",
            ),
            T::ByteRate => (
                "Byte rate",
                "Bytes a second of audio occupies — sample rate × block align. A player uses it \
                 to turn a byte offset into a timestamp without decoding.",
            ),
            T::ValidBits => (
                "Valid bits",
                "How many bits of each sample actually carry signal, when the container is \
                 wider than the recording. 24-bit audio is often stored in 32-bit words with 8 \
                 bits unused.",
            ),
            T::ChannelMask => (
                "Channel mask",
                "A bit per speaker position, saying which ones this file's channels are meant \
                 for — front left, front right, centre, LFE and so on. Set by the extensible \
                 WAVE format; absent from plain mono and stereo files.",
            ),
            T::RiffBody => (
                "Body",
                "Size the RIFF header claims for the file's contents. If it disagrees with the \
                 file on disk the file was truncated, or was still being written when it was \
                 copied.",
            ),

            T::Chunks => (
                "Chunks",
                "Every block in the RIFF file, with where it starts and how big it is. data \
                 holds the audio; fmt describes it; the rest is metadata. Gaps or overlaps \
                 between them mean a malformed file.",
            ),

            // ---- broadcast wave ----------------------------------------
            T::BwfOriginator => (
                "Originator",
                "What made the file — the recorder model, or the software that wrote it. Set by \
                 the device, so it is a reliable record of the capture chain.",
            ),
            T::BwfReference => (
                "Reference",
                "The originator's own identifier for this recording, usually encoding the \
                 device and the time of day. Used to tie a file back to a session log.",
            ),
            T::BwfOriginated => (
                "Originated",
                "Date and time the recording was made, written by the recorder. Unlike the \
                 file's modified time this survives copying, so it is what to trust for when \
                 the audio was captured.",
            ),
            T::BwfTimeRef => (
                "Time ref",
                "Where this file sits on the session's timeline, as a sample count since \
                 midnight. This is what lets an editor drop wild takes into sync without \
                 slating them — the timecode, at sample precision.",
            ),
            T::BwfUmid => (
                "UMID",
                "Unique Material Identifier: a globally unique label for this piece of \
                 material, from the SMPTE standard. Shown abbreviated; it is long.",
            ),
            T::BwfStoredLoudness => (
                "Stored loudness",
                "Loudness the recorder or encoder wrote into the file when it was made. It is a \
                 claim, not a measurement of what is here now — compare it with the Loudness \
                 card below, which is measured from the actual audio.",
            ),

            // ---- loudness ----------------------------------------------
            T::Integrated => (
                "Integrated loudness",
                "Loudness averaged over the whole file, in LUFS, with quiet passages gated out \
                 so silence does not drag the number down. This is the single figure delivery \
                 specs are written against.",
            ),
            T::LoudnessRange => (
                "Loudness range",
                "How much the loudness varies over the file, in LU. A small range means \
                 consistent level — speech, a mastered pop track. A large one means dynamics \
                 that a broadcast chain may compress.",
            ),
            T::MaxMomentary => (
                "Max momentary",
                "The loudest the file gets measured over a 400 ms window. It catches short \
                 stabs and transients that the integrated figure averages away.",
            ),
            T::MaxShortTerm => (
                "Max short-term",
                "The loudest the file gets measured over a 3 s window. Between momentary and \
                 integrated: it reflects how loud a passage feels rather than a single hit.",
            ),
            T::Correlation => (
                "Correlation",
                "How alike the two channels are, from +1 to −1. Near +1 is effectively mono; \
                 around 0 is a wide stereo image; negative means the channels fight each other \
                 and will partly cancel if the mix is folded to mono.",
            ),
            T::AgainstTargets => (
                "Against targets",
                "How far this file's integrated loudness sits from the usual delivery targets. \
                 Positive means it is louder than the target and would be turned down; negative \
                 means quieter, and would be turned up.",
            ),
            T::TargetDelta => (
                "Distance to target",
                "The gain change that would land this file on that target, in LU. Green is \
                 within 1 LU and effectively on spec, amber within 3, plain beyond that. \
                 Applying it shifts true peak by the same amount — check Headroom first.",
            ),
            T::Headroom => (
                "Headroom",
                "How far the loudest true peak sits below full scale. It is what you have to \
                 turn up before clipping. Broadcast specs usually ask for at least 1 dB, so a \
                 figure under that is worth fixing.",
            ),
            T::CrestFactor => (
                "Crest factor",
                "The gap between peak and RMS: how spiky the signal is. A large crest factor \
                 means sharp transients over a quiet average — unprocessed drums, speech. A \
                 small one means heavy compression or limiting.",
            ),

            // ---- levels ------------------------------------------------
            T::PeakLevel => (
                "Peak",
                "The largest single sample in the channel, in dBFS. It is what a conventional \
                 meter shows, and it can miss overshoots that appear between samples — which is \
                 what true peak is for.",
            ),
            T::TruePeakLevel => (
                "True peak",
                "The real maximum of the waveform, including the overshoots that appear between \
                 samples once the signal is reconstructed. It can exceed sample peak by a \
                 decibel or more, and it is what a D/A converter or a lossy encoder actually \
                 has to cope with.",
            ),
            T::RmsLevel => (
                "RMS",
                "The average energy of the channel, in dBFS. Closer to how loud it seems than \
                 peak is, though without the frequency weighting the LUFS figures apply.",
            ),
            T::DcOffset => (
                "DC offset",
                "How far the waveform's average sits from zero. It should be near nothing; a \
                 real offset wastes headroom, can thump at edits, and usually means a faulty \
                 input stage. Above 0.01 is flagged.",
            ),
            T::Clipping => (
                "Clipping",
                "Samples sitting at or beyond full scale, and how many unbroken runs of them \
                 there are. Long runs mean the waveform was flattened and detail is gone; a few \
                 isolated samples are usually harmless.",
            ),

            // ---- analysis ----------------------------------------------
            T::AnWindow => (
                "Window",
                "The three choices behind the spectrogram: how many samples go into each FFT, \
                 which window function shapes them, and how far consecutive frames overlap.",
            ),
            T::AnHop => (
                "Hop",
                "How far the analysis window advances between frames, in samples and in time. \
                 It follows from the window size and the overlap, and it sets the spectrogram's \
                 time resolution — one column per hop.",
            ),
            T::Resolution => (
                "Resolution",
                "What the current settings buy you: the width of one frequency bin, the span of \
                 audio in each frame, and the step between frames. Frequency and time detail \
                 trade against each other — a longer window sharpens pitch and blurs timing.",
            ),
            T::Reassignment => (
                "Reassignment",
                "Moves each bin's energy to the frequency and time it actually came from, \
                 instead of smearing it across the window. Tones come out as thin lines and \
                 clicks as sharp edges. Costs three FFTs per frame instead of one.",
            ),
            T::DetailTile => (
                "Detail tile",
                "When you zoom past the resolution of the whole-file spectrogram, a finer strip \
                 is computed just for what is on screen. This is its width in columns and how \
                 many samples each one covers.",
            ),
            T::ViewRange => (
                "View",
                "The stretch of the file currently drawn. Scroll or zoom to move it; F fits the \
                 whole file, or the range if you have one.",
            ),
            T::ViewSpan => (
                "Span",
                "How much time the view covers. Together with the hop it tells you whether the \
                 spectrogram on screen is showing every frame or a decimated summary.",
            ),
            T::FloorCeiling => (
                "Floor / ceiling",
                "The dB range the colour map is stretched across. Levels at or below the floor \
                 are drawn darkest, at or above the ceiling brightest. Narrow the gap to bring \
                 out quiet detail; widen it to calm a busy picture.",
            ),

            // ---- cursor ------------------------------------------------
            T::Playhead => (
                "Playhead",
                "Where playback has reached. Click anywhere in the waveform or spectrogram to \
                 move it; Space starts and stops.",
            ),
            T::PointerReadout => (
                "Pointer",
                "Time and, over the spectrogram, frequency and level under the pointer right \
                 now. The quickest way to put a number on something you can see.",
            ),
            T::Selection => (
                "Selection",
                "The highlight from your last drag, and how long it is. F zooms to it; Esc \
                 clears it. It is transient — the next click drops it.",
            ),
            T::RulerRange => (
                "Range",
                "The span kept on the ruler after a drag, with handles you can pull to adjust. \
                 Unlike the selection it survives clicking elsewhere, and it is what the loop \
                 plays.",
            ),
            T::LoopToggle => (
                "Loop",
                "Whether playback repeats the ruler range instead of running to the end of the \
                 file. L toggles it; it needs a range to loop.",
            ),

            // ---- settings: cards ---------------------------------------
            T::PanesCard => (
                "Panes",
                "Which of the three views are drawn and how the window is divided between \
                 them.",
            ),
            T::SpectrogramCard => (
                "Spectrogram",
                "Frequency against time, colour for level. It is built by cutting the audio \
                 into overlapping frames and taking an FFT of each, so every setting here is \
                 some corner of that trade between time and frequency detail.",
            ),
            T::WaveformCard => (
                "Waveform",
                "Amplitude against time — the shape of the signal itself. Good for finding \
                 edits, transients and level problems; blind to what frequencies are in them.",
            ),
            T::SpectrumCard => (
                "Spectrum",
                "The frequency content of whatever is playing right now, updated live. A slice \
                 through the spectrogram at the playhead, read as a graph.",
            ),
            T::FilesCard => (
                "Files",
                "What the app remembers about the files you open: the history behind the \
                 Recent menu, and which file it reopens when it starts. All of it is kept on \
                 this machine and nothing is sent anywhere.",
            ),
            T::KeysCard => (
                "Keys",
                "Every keyboard and mouse shortcut. F1 toggles the help mode you are reading \
                 this in.",
            ),
            T::AboutCard => (
                "About",
                "Which build is running and where it came from. Dev builds show the git \
                 description they were stamped with.",
            ),

            // ---- settings: panes ---------------------------------------
            T::ShowPanes => (
                "Show",
                "Which views are drawn. The waveform shows amplitude against time, the \
                 spectrogram frequency against time, and the spectrum the live content of \
                 whatever is playing.",
            ),
            T::MergeViews => (
                "Merge",
                "Draws the waveform over the spectrogram as one pane instead of stacking two. \
                 Useful for lining a transient up with what it looks like in frequency; needs \
                 both views on.",
            ),
            T::MergeWaveOpacity => (
                "Waveform opacity",
                "How solid the overlaid waveform is while merged. Drop it when the spectrogram \
                 underneath is what you are reading.",
            ),
            T::MergeSpecOpacity => (
                "Spectrogram opacity",
                "How solid the spectrogram is under the merged waveform. Drop it to let the \
                 waveform's shape dominate.",
            ),

            // ---- settings: spectrogram ---------------------------------
            T::WindowSize => (
                "Window size",
                "Samples per FFT, and the central trade-off in the spectrogram. Larger windows \
                 separate close pitches but blur when things happen; smaller ones pin down \
                 timing at the cost of frequency detail. 2048 is a reasonable default for \
                 speech and music at 48 kHz.",
            ),
            T::Overlap => (
                "Overlap",
                "How much each analysis frame shares with the one before. More overlap means \
                 more columns and a smoother picture in time, at proportionally more work — \
                 75% is the usual compromise, 0% the cheapest and blockiest.",
            ),
            T::WindowKind => (
                "Window",
                "The shape each frame is multiplied by before its FFT, which controls how much \
                 a strong tone leaks into neighbouring bins. Hann is the safe default; \
                 Blackman-Harris suppresses leakage further at the cost of a wider main lobe; \
                 a rectangular window does not taper at all.",
            ),
            T::ColourMap => (
                "Colour map",
                "How level maps to colour. Inferno and Magma are perceptually even, so equal \
                 steps in dB look like equal steps in brightness. Grey is the most honest for \
                 judging relative level; Custom lets you set the stops yourself.",
            ),
            T::CustomColours => (
                "Custom colours",
                "The three stops the Custom map interpolates between: quiet, medium and loud. \
                 Anything below the quiet colour is drawn black.",
            ),
            T::Contrast => (
                "Contrast",
                "Gamma applied to the colour map. Above 1 darkens quiet material and cleans up \
                 a noisy picture; below 1 lifts it and brings out low-level detail like room \
                 tone or reverb tails.",
            ),
            T::FrequencyAxis => (
                "Frequency axis",
                "Linear spreads every hertz equally, which favours the top octaves. \
                 Logarithmic gives each octave the same height, which is how pitch is actually \
                 heard — better for music, speech and anything with harmonics.",
            ),
            T::Floor => (
                "Floor",
                "The level drawn as the darkest colour. Everything quieter is clamped to it, so \
                 raising the floor hides noise and lowering it exposes more of what is down \
                 there.",
            ),
            T::Ceiling => (
                "Ceiling",
                "The level drawn as the brightest colour. Lower it when the picture is too dark \
                 overall; raise it when loud material is saturating into a flat block.",
            ),
            T::LowestFrequency => (
                "Lowest frequency",
                "Where the logarithmic axis starts. A log scale cannot reach zero, so this sets \
                 how much of the bottom end is given room — raise it to spend the height on the \
                 range that matters for speech.",
            ),

            // ---- settings: waveform ------------------------------------
            T::WaveOverlays => (
                "Overlays",
                "RMS draws the average energy inside the peak outline, so you can see how dense \
                 the signal is rather than just how tall. The dB scale puts labelled gridlines \
                 on the waveform.",
            ),
            T::WaveColour => (
                "Colour",
                "The waveform's colour. The RMS overlay is drawn as a lighter tint of whatever \
                 you pick, so the two stay legible together.",
            ),
            T::VerticalZoom => (
                "Vertical zoom",
                "Stretches the waveform vertically without changing the audio, to see detail in \
                 quiet material. Alt+Shift+wheel over the waveform does the same.",
            ),
            T::WaveHeight => (
                "Height",
                "Share of the pane given to the waveform, with the spectrogram taking the rest. \
                 Dragging the divider between them does the same. Not used while the two are \
                 merged.",
            ),

            // ---- settings: spectrum ------------------------------------
            T::SpectrumSize => (
                "FFT size",
                "Samples per frame in the live spectrum. Larger resolves close frequencies \
                 better but responds more slowly, which matters more here than in the \
                 spectrogram because this one is tracking playback in real time.",
            ),
            T::SpectrumAveraging => (
                "Averaging",
                "How much of the previous frame is carried into the next. Higher settles the \
                 display and makes steady tones easy to read; lower reacts instantly but \
                 flickers.",
            ),

            // ---- settings: files ---------------------------------------
            T::RememberFiles => (
                "History",
                "Keeps the files you open in a list, newest first, so the Recent menu can \
                 reopen them and the app can pick up where you left off. Switch it off and the \
                 list is cleared and nothing further is recorded — the app then starts empty.",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every topic has copy, and none of it is a placeholder. The match is
    /// exhaustive by construction, so this catches the other failure: a
    /// variant added with the body left empty or stubbed.
    #[test]
    fn every_topic_has_real_copy() {
        for topic in ALL {
            let (title, body) = topic.text();
            assert!(!title.is_empty(), "{topic:?} has no title");
            assert!(body.len() > 40, "{topic:?} has a stub body: {body:?}",);
            assert!(
                body.ends_with('.') || body.ends_with('?'),
                "{topic:?} body is not a finished sentence: {body:?}",
            );
        }
    }

    /// Titles are headings, not sentences, and the popover gives them a line
    /// of their own.
    #[test]
    fn titles_are_short_and_unpunctuated() {
        for topic in ALL {
            let (title, _) = topic.text();
            assert!(title.len() <= 28, "{topic:?} title is too long: {title:?}");
            assert!(!title.ends_with('.'), "{topic:?} title ends in a full stop");
        }
    }

    /// Every topic, for the copy checks below.
    ///
    /// A slice, not a sized array: the length is one more thing to keep in
    /// step for no gain. Nothing here can go missing silently anyway — the
    /// match in `text` is exhaustive, so the compiler is what actually
    /// guarantees a new variant gets copy.
    const ALL: &[Topic] = &[
        Topic::FileCard,
        Topic::WaveHeaderCard,
        Topic::BwfCard,
        Topic::TagsCard,
        Topic::MarkersCard,
        Topic::LoudnessCard,
        Topic::LevelsCard,
        Topic::AnalysisCard,
        Topic::CursorCard,
        Topic::Container,
        Topic::Codec,
        Topic::SampleRate,
        Topic::Channels,
        Topic::BitDepth,
        Topic::Duration,
        Topic::Frames,
        Topic::FileSize,
        Topic::BitRate,
        Topic::InMemory,
        Topic::Modified,
        Topic::WavFormat,
        Topic::BlockAlign,
        Topic::ByteRate,
        Topic::ValidBits,
        Topic::ChannelMask,
        Topic::RiffBody,
        Topic::Chunks,
        Topic::BwfOriginator,
        Topic::BwfReference,
        Topic::BwfOriginated,
        Topic::BwfTimeRef,
        Topic::BwfUmid,
        Topic::BwfStoredLoudness,
        Topic::Integrated,
        Topic::LoudnessRange,
        Topic::MaxMomentary,
        Topic::MaxShortTerm,
        Topic::Correlation,
        Topic::AgainstTargets,
        Topic::TargetDelta,
        Topic::Headroom,
        Topic::CrestFactor,
        Topic::PeakLevel,
        Topic::TruePeakLevel,
        Topic::RmsLevel,
        Topic::DcOffset,
        Topic::Clipping,
        Topic::AnWindow,
        Topic::AnHop,
        Topic::Resolution,
        Topic::Reassignment,
        Topic::DetailTile,
        Topic::ViewRange,
        Topic::ViewSpan,
        Topic::FloorCeiling,
        Topic::Playhead,
        Topic::PointerReadout,
        Topic::Selection,
        Topic::RulerRange,
        Topic::LoopToggle,
        Topic::PanesCard,
        Topic::SpectrogramCard,
        Topic::WaveformCard,
        Topic::SpectrumCard,
        Topic::FilesCard,
        Topic::KeysCard,
        Topic::AboutCard,
        Topic::ShowPanes,
        Topic::MergeViews,
        Topic::MergeWaveOpacity,
        Topic::MergeSpecOpacity,
        Topic::WindowSize,
        Topic::Overlap,
        Topic::WindowKind,
        Topic::ColourMap,
        Topic::CustomColours,
        Topic::Contrast,
        Topic::FrequencyAxis,
        Topic::Floor,
        Topic::Ceiling,
        Topic::LowestFrequency,
        Topic::WaveOverlays,
        Topic::WaveColour,
        Topic::VerticalZoom,
        Topic::WaveHeight,
        Topic::SpectrumSize,
        Topic::SpectrumAveraging,
        Topic::RememberFiles,
    ];
}
