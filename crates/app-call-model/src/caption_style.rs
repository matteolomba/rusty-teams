#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptionSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl CaptionSize {
    pub const ALL: [CaptionSize; 3] = [CaptionSize::Small, CaptionSize::Medium, CaptionSize::Large];

    pub fn label(self) -> String {
        match self {
            CaptionSize::Small => app_base::ui_locale::tr("ui.calls.caption.small"),
            CaptionSize::Medium => app_base::ui_locale::tr("ui.calls.caption.medium"),
            CaptionSize::Large => app_base::ui_locale::tr("ui.calls.caption.large"),
        }
    }

    pub fn text_size(self) -> f32 {
        match self {
            CaptionSize::Small => 13.,
            CaptionSize::Medium => 16.,
            CaptionSize::Large => 21.,
        }
    }

    pub fn meta(self) -> &'static str {
        match self {
            CaptionSize::Small => "small",
            CaptionSize::Medium => "medium",
            CaptionSize::Large => "large",
        }
    }

    pub fn from_meta(value: &str) -> CaptionSize {
        match value {
            "small" => CaptionSize::Small,
            "large" => CaptionSize::Large,
            _ => CaptionSize::Medium,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptionPosition {
    #[default]
    Bottom,
    Top,
}

impl CaptionPosition {
    pub const ALL: [CaptionPosition; 2] = [CaptionPosition::Bottom, CaptionPosition::Top];

    pub fn label(self) -> String {
        match self {
            CaptionPosition::Bottom => app_base::ui_locale::tr("ui.calls.caption.position_bottom"),
            CaptionPosition::Top => app_base::ui_locale::tr("ui.calls.caption.position_top"),
        }
    }

    pub fn meta(self) -> &'static str {
        match self {
            CaptionPosition::Bottom => "bottom",
            CaptionPosition::Top => "top",
        }
    }

    pub fn from_meta(value: &str) -> CaptionPosition {
        match value {
            "top" => CaptionPosition::Top,
            _ => CaptionPosition::Bottom,
        }
    }
}
