//! The app's icons: Phosphor (MIT, the same on every platform), each picked to match the SF Symbol the
//! Swift app shows in that place. Served to GPUI's `svg()` as `icons/<Icon>.svg` (`-fill` for the filled
//! variant); GPUI paints an SVG as a mask in the current text colour.
use gpui::{AssetSource, Hsla, Pixels, SharedString, Svg, prelude::*, svg};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Search,
    Bell,
    BellOff,
    Chat,
    People,
    Calendar,
    Phone,
    PhoneOff,
    File,
    Folder,
    Gear,
    Refresh,
    Close,
    Plus,
    Compose,
    Filter,
    More,
    Send,
    Smile,
    Gif,
    Apps,
    At,
    Clip,
    Alert,
    Bold,
    Italic,
    Underline,
    Strike,
    ListBullet,
    ListNumber,
    Code,
    Link,
    Quote,
    Video,
    VideoOff,
    Mic,
    MicOff,
    Screen,
    Speaker,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    ChevronUp,
    Check,
    CheckDouble,
    Eye,
    Pin,
    Reply,
    Pencil,
    Trash,
    Copy,
    Download,
    External,
    Image,
    Play,
    Pause,
    Stop,
    Voicemail,
    ArrowLeft,
    Info,
    WifiOff,
    Star,
    Lock,
    Globe,
    Logout,
    Sun,
    Moon,
    Clock,
    Location,
    Record,
    Hand,
    Grid,
    Archive,
    UserPlus,
    UserMinus,
    Leave,
    Hide,
    Sticker,
    Format,
    Bot,
    Thread,
    Sort,
    Upload,
    FolderFill,
    FolderPlus,
    Hash,
    Branch,
    Person,
    Repeat,
    Question,
    MinusCircle,
    Envelope,
    Airplane,
    Chip,
    Contrast,
    PhoneIncoming,
    PhoneOutgoing,
    Antenna,
    AntennaOff,
    ZoomIn,
    ZoomOut,
    Scope,
    ArrowDown,
    BellFill,
    FontColor,
    Highlighter,
    IndentLess,
    IndentMore,
    ClearFormat,
    SpeakerOff,
    ScreenOff,
    LayoutSpeaker,
    LayoutSide,
    LayoutFocus,
    Wave,
    Devices,
    AppWindow,
    Spinner,
}

impl Icon {
    pub fn path(self) -> SharedString {
        format!("icons/{self:?}.svg").into()
    }
    /// The filled variant, as Swift's `.symbolVariant(.fill)` (the rail's selected section, …).
    pub fn fill_path(self) -> SharedString {
        format!("icons/{self:?}-fill.svg").into()
    }

    /// Phosphor's regular and fill drawings for this icon, picked to match the SF Symbol Swift shows there.
    fn phosphor(self) -> (&'static str, &'static str) {
        use phosphor_svgs::style::fill as f;
        use phosphor_svgs::style::regular::*;
        let (regular, fill) = match self {
            Icon::Search => (MAGNIFYING_GLASS, f::MAGNIFYING_GLASS),
            Icon::Bell => (BELL, f::BELL),
            Icon::BellOff => (BELL_SLASH, f::BELL_SLASH),
            Icon::BellFill => (BELL, f::BELL),
            Icon::Chat => (CHAT, f::CHAT),
            Icon::People => (USERS_THREE, f::USERS_THREE),
            Icon::Calendar => (CALENDAR_BLANK, f::CALENDAR_BLANK),
            Icon::Phone => (PHONE, f::PHONE),
            Icon::PhoneOff => (PHONE_DISCONNECT, f::PHONE_DISCONNECT),
            Icon::File => (FILE, f::FILE),
            Icon::Folder => (FOLDER, f::FOLDER),
            Icon::FolderFill => (FOLDER, f::FOLDER),
            Icon::FolderPlus => (FOLDER_PLUS, f::FOLDER_PLUS),
            Icon::Gear => (GEAR_SIX, f::GEAR_SIX),
            Icon::Refresh => (ARROW_CLOCKWISE, f::ARROW_CLOCKWISE),
            Icon::Close => (X, f::X),
            Icon::Plus => (PLUS, f::PLUS),
            Icon::Compose => (NOTE_PENCIL, f::NOTE_PENCIL),
            Icon::Filter => (FUNNEL_SIMPLE, f::FUNNEL_SIMPLE),
            Icon::More => (DOTS_THREE, f::DOTS_THREE),
            Icon::Send => (PAPER_PLANE_RIGHT, f::PAPER_PLANE_RIGHT),
            Icon::Smile => (SMILEY, f::SMILEY),
            Icon::Gif => (GIF, f::GIF),
            Icon::Apps => (SQUARES_FOUR, f::SQUARES_FOUR),
            Icon::At => (AT, f::AT),
            Icon::Clip => (PAPERCLIP, f::PAPERCLIP),
            Icon::Alert => (WARNING_CIRCLE, f::WARNING_CIRCLE),
            Icon::Bold => (TEXT_B, f::TEXT_B),
            Icon::Italic => (TEXT_ITALIC, f::TEXT_ITALIC),
            Icon::Underline => (TEXT_UNDERLINE, f::TEXT_UNDERLINE),
            Icon::Strike => (TEXT_STRIKETHROUGH, f::TEXT_STRIKETHROUGH),
            Icon::ListBullet => (LIST_BULLETS, f::LIST_BULLETS),
            Icon::ListNumber => (LIST_NUMBERS, f::LIST_NUMBERS),
            Icon::Code => (CODE, f::CODE),
            Icon::Link => (LINK, f::LINK),
            Icon::Quote => (QUOTES, f::QUOTES),
            Icon::Video => (VIDEO_CAMERA, f::VIDEO_CAMERA),
            Icon::VideoOff => (VIDEO_CAMERA_SLASH, f::VIDEO_CAMERA_SLASH),
            Icon::Mic => (MICROPHONE, f::MICROPHONE),
            Icon::MicOff => (MICROPHONE_SLASH, f::MICROPHONE_SLASH),
            Icon::Screen => (SCREENCAST, f::SCREENCAST),
            Icon::ScreenOff => (PROHIBIT, f::PROHIBIT),
            Icon::Speaker => (SPEAKER_HIGH, f::SPEAKER_HIGH),
            Icon::SpeakerOff => (SPEAKER_SLASH, f::SPEAKER_SLASH),
            Icon::ChevronDown => (CARET_DOWN, f::CARET_DOWN),
            Icon::ChevronRight => (CARET_RIGHT, f::CARET_RIGHT),
            Icon::ChevronLeft => (CARET_LEFT, f::CARET_LEFT),
            Icon::ChevronUp => (CARET_UP, f::CARET_UP),
            Icon::Check => (CHECK, f::CHECK),
            Icon::CheckDouble => (CHECKS, f::CHECKS),
            Icon::Eye => (EYE, f::EYE),
            Icon::Hide => (EYE_SLASH, f::EYE_SLASH),
            Icon::Pin => (PUSH_PIN, f::PUSH_PIN),
            Icon::Reply => (ARROW_BEND_UP_LEFT, f::ARROW_BEND_UP_LEFT),
            Icon::Pencil => (PENCIL_SIMPLE, f::PENCIL_SIMPLE),
            Icon::Trash => (TRASH, f::TRASH),
            Icon::Copy => (COPY, f::COPY),
            Icon::Download => (DOWNLOAD_SIMPLE, f::DOWNLOAD_SIMPLE),
            Icon::External => (ARROW_SQUARE_OUT, f::ARROW_SQUARE_OUT),
            Icon::Image => (IMAGE, f::IMAGE),
            Icon::Play => (PLAY, f::PLAY),
            Icon::Pause => (PAUSE, f::PAUSE),
            Icon::Stop => (STOP, f::STOP),
            Icon::Voicemail => (VOICEMAIL, f::VOICEMAIL),
            Icon::ArrowLeft => (ARROW_LEFT, f::ARROW_LEFT),
            Icon::ArrowDown => (ARROW_DOWN, f::ARROW_DOWN),
            Icon::Info => (INFO, f::INFO),
            Icon::WifiOff => (WIFI_SLASH, f::WIFI_SLASH),
            Icon::Star => (STAR, f::STAR),
            Icon::Lock => (LOCK, f::LOCK),
            Icon::Globe => (GLOBE, f::GLOBE),
            Icon::Logout => (SIGN_OUT, f::SIGN_OUT),
            Icon::Sun => (SUN, f::SUN),
            Icon::Moon => (MOON, f::MOON),
            Icon::Clock => (CLOCK, f::CLOCK),
            Icon::Location => (MAP_PIN, f::MAP_PIN),
            Icon::Record => (RECORD, f::RECORD),
            Icon::Hand => (HAND, f::HAND),
            Icon::Grid => (GRID_FOUR, f::GRID_FOUR),
            Icon::Archive => (ARCHIVE, f::ARCHIVE),
            Icon::UserPlus => (USER_PLUS, f::USER_PLUS),
            Icon::UserMinus => (USER_MINUS, f::USER_MINUS),
            Icon::Leave => (DOOR_OPEN, f::DOOR_OPEN),
            Icon::Sticker => (STICKER, f::STICKER),
            Icon::Format => (TEXT_AA, f::TEXT_AA),
            Icon::Bot => (ROBOT, f::ROBOT),
            Icon::Thread => (CHATS, f::CHATS),
            Icon::Sort => (ARROWS_DOWN_UP, f::ARROWS_DOWN_UP),
            Icon::Upload => (FILE_ARROW_UP, f::FILE_ARROW_UP),
            Icon::Hash => (HASH, f::HASH),
            Icon::Branch => (GIT_BRANCH, f::GIT_BRANCH),
            Icon::Person => (USER, f::USER),
            Icon::Repeat => (REPEAT, f::REPEAT),
            Icon::Question => (QUESTION, f::QUESTION),
            Icon::MinusCircle => (MINUS_CIRCLE, f::MINUS_CIRCLE),
            Icon::Envelope => (ENVELOPE_SIMPLE, f::ENVELOPE_SIMPLE),
            Icon::Airplane => (AIRPLANE, f::AIRPLANE),
            Icon::Chip => (CPU, f::CPU),
            Icon::Contrast => (CIRCLE_HALF, f::CIRCLE_HALF),
            Icon::PhoneIncoming => (PHONE_INCOMING, f::PHONE_INCOMING),
            Icon::PhoneOutgoing => (PHONE_OUTGOING, f::PHONE_OUTGOING),
            Icon::Antenna => (BROADCAST, f::BROADCAST),
            Icon::AntennaOff => (CELL_SIGNAL_SLASH, f::CELL_SIGNAL_SLASH),
            Icon::ZoomIn => (MAGNIFYING_GLASS_PLUS, f::MAGNIFYING_GLASS_PLUS),
            Icon::ZoomOut => (MAGNIFYING_GLASS_MINUS, f::MAGNIFYING_GLASS_MINUS),
            Icon::Scope => (CORNERS_OUT, f::CORNERS_OUT),
            Icon::FontColor => (TEXT_A_UNDERLINE, f::TEXT_A_UNDERLINE),
            Icon::Highlighter => (HIGHLIGHTER, f::HIGHLIGHTER),
            Icon::IndentLess => (TEXT_OUTDENT, f::TEXT_OUTDENT),
            Icon::IndentMore => (TEXT_INDENT, f::TEXT_INDENT),
            Icon::ClearFormat => (TEXT_T_SLASH, f::TEXT_T_SLASH),
            Icon::LayoutSpeaker => (PRESENTATION, f::PRESENTATION),
            Icon::LayoutSide => (SIDEBAR_SIMPLE, f::SIDEBAR_SIMPLE),
            Icon::LayoutFocus => (FRAME_CORNERS, f::FRAME_CORNERS),
            Icon::Wave => (WAVEFORM, f::WAVEFORM),
            Icon::Devices => (DEVICES, f::DEVICES),
            Icon::AppWindow => (APP_WINDOW, f::APP_WINDOW),
            Icon::Spinner => (CIRCLE_NOTCH, f::CIRCLE_NOTCH),
        };
        // Swift draws these from ".fill" symbols wherever they appear.
        if matches!(self, Icon::BellFill | Icon::FolderFill | Icon::MicOff | Icon::PhoneOff | Icon::Pin | Icon::Send) {
            (fill, fill)
        } else {
            (regular, fill)
        }
    }

    const ALL: &[Icon] = &[Icon::Search, Icon::Bell, Icon::BellOff, Icon::BellFill, Icon::Chat, Icon::People, Icon::Calendar, Icon::Phone, Icon::PhoneOff, Icon::File, Icon::Folder, Icon::FolderFill, Icon::FolderPlus, Icon::Gear, Icon::Refresh, Icon::Close, Icon::Plus, Icon::Compose, Icon::Filter, Icon::More, Icon::Send, Icon::Smile, Icon::Gif, Icon::Apps, Icon::At, Icon::Clip, Icon::Alert, Icon::Bold, Icon::Italic, Icon::Underline, Icon::Strike, Icon::ListBullet, Icon::ListNumber, Icon::Code, Icon::Link, Icon::Quote, Icon::Video, Icon::VideoOff, Icon::Mic, Icon::MicOff, Icon::Screen, Icon::ScreenOff, Icon::Speaker, Icon::SpeakerOff, Icon::ChevronDown, Icon::ChevronRight, Icon::ChevronLeft, Icon::ChevronUp, Icon::Check, Icon::CheckDouble, Icon::Eye, Icon::Hide, Icon::Pin, Icon::Reply, Icon::Pencil, Icon::Trash, Icon::Copy, Icon::Download, Icon::External, Icon::Image, Icon::Play, Icon::Pause, Icon::Stop, Icon::Voicemail, Icon::ArrowLeft, Icon::ArrowDown, Icon::Info, Icon::WifiOff, Icon::Star, Icon::Lock, Icon::Globe, Icon::Logout, Icon::Sun, Icon::Moon, Icon::Clock, Icon::Location, Icon::Record, Icon::Hand, Icon::Grid, Icon::Archive, Icon::UserPlus, Icon::UserMinus, Icon::Leave, Icon::Sticker, Icon::Format, Icon::Bot, Icon::Thread, Icon::Sort, Icon::Upload, Icon::Hash, Icon::Branch, Icon::Person, Icon::Repeat, Icon::Question, Icon::MinusCircle, Icon::Envelope, Icon::Airplane, Icon::Chip, Icon::Contrast, Icon::PhoneIncoming, Icon::PhoneOutgoing, Icon::Antenna, Icon::AntennaOff, Icon::ZoomIn, Icon::ZoomOut, Icon::Scope, Icon::FontColor, Icon::Highlighter, Icon::IndentLess, Icon::IndentMore, Icon::ClearFormat, Icon::LayoutSpeaker, Icon::LayoutSide, Icon::LayoutFocus, Icon::Wave, Icon::Devices, Icon::AppWindow, Icon::Spinner];

    fn from_name(name: &str) -> Option<Icon> {
        Self::ALL.iter().copied().find(|i| format!("{i:?}") == name)
    }
}

/// `icons/<Icon>.svg` or `icons/<Icon>-fill.svg`.
fn svg_for(name: &str) -> Option<&'static str> {
    match name.strip_suffix("-fill") {
        Some(base) => Icon::from_name(base).map(|i| i.phosphor().1),
        None => Icon::from_name(name).map(|i| i.phosphor().0),
    }
}

fn brand_svg(name: &str) -> Option<String> {
    // "mark-28x26": GPUI rasterises an SVG at twice its declared size, so declaring the size it is shown
    // at keeps it sharp on Retina; the shape's own 644 × 600 would be shrunk without smoothing.
    let (name, (width, height)) = match name.rsplit_once('-') {
        Some((base, dims)) if dims.contains('x') => {
            let (w, h) = dims.split_once('x')?;
            (base, (w.parse::<f32>().ok()?, h.parse::<f32>().ok()?))
        }
        _ => (name, (644., 600.)),
    };
    let (bubble, figure, people) = match name {
        "mark" => ("url(#violet)", "#FFFFFF", true),
        "mark-white" => ("#FFFFFF", "#4B4FB8", true),
        "bubble-white" => ("#FFFFFF", "#FFFFFF", false),
        _ => return None,
    };
    let people = if people {
        format!(
            r##"<g fill="{figure}" fill-opacity="0.72"><circle cx="428" cy="380" r="60"/><path d="M298,604 C298,522 356,470 428,470 C500,470 558,522 558,604 Z"/></g><g fill="{figure}" stroke="{bubble}" stroke-width="20" paint-order="stroke"><circle cx="592" cy="408" r="66"/><path d="M448,644 C448,556 512,504 592,504 C672,504 736,556 736,644 Z"/></g>"##
        )
    } else {
        String::new()
    };
    Some(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="190 236 644 600" width="{width}" height="{height}" preserveAspectRatio="xMidYMid meet"><defs><linearGradient id="violet" x1="0" y1="236" x2="0" y2="836" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="#7B7FE0"/><stop offset="1" stop-color="#4B4FB8"/></linearGradient></defs><g fill="{bubble}"><rect x="190" y="236" width="644" height="464" rx="132"/><path d="M300,640 L300,700 C300,764 276,806 236,836 C326,834 406,794 452,700 L452,640 Z"/></g>{people}</svg>"##
    ))
}

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix("brand/").and_then(|p| p.strip_suffix(".svg")) {
            return Ok(brand_svg(name).map(|s| Cow::Owned(s.into_bytes())));
        }
        Ok(path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"))
            .and_then(svg_for)
            .map(|s| Cow::Borrowed(s.as_bytes())))
    }
    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(if path.starts_with("icons") {
            Icon::ALL.iter().map(|i| i.path()).collect()
        } else {
            Vec::new()
        })
    }
}

/// An icon at `size` points in `color`.
pub fn icon(icon: Icon, size: Pixels, color: Hsla) -> Svg {
    svg().path(icon.path()).size(size).flex_none().text_color(color)
}

/// The same icon's filled variant (Swift's `.symbolVariant(.fill)`).
pub fn icon_fill(icon: Icon, size: Pixels, color: Hsla) -> Svg {
    svg().path(icon.fill_path()).size(size).flex_none().text_color(color)
}
