//! Chrome icons drawn as Nerd Font glyphs, with a themed-icon fallback.
//!
//! mithshell's own controls (media transport, volume, battery, window
//! buttons, ...) used to be named freedesktop icons resolved through whatever
//! GTK icon theme happened to be installed. That made the shell's appearance
//! depend on an unrelated user setting, and two of the names it relied on are
//! not reliably available at all: `xsi-battery-*` ships in `xapp-symbolic-icons`
//! rather than any icon theme, and `system-suspend-symbolic` is absent from
//! Adwaita.
//!
//! Rendering those icons as text glyphs removes both problems. Only *our*
//! chrome moves; icons supplied by other programs (tray items, notification
//! `app_icon`, MPRIS players, TarraGon results) stay `gtk::Image`s because
//! their names and paths are chosen by the sending application.
//!
//! # Why the Font Awesome range
//!
//! Every codepoint below sits in `U+F000..=U+F2FF`. Nerd Fonts v3 relocated the
//! Material Design Icons block from `U+F500..=U+FD46` to `U+F0001..=U+F1AF0`,
//! so an MDI codepoint renders on either v2 or v3 patched fonts but never
//! both. The Font Awesome block was left in place by that migration and is
//! byte-identical across the two generations, which lets one table serve any
//! patched font a user happens to have. It also avoids collisions with
//! Monocraft, the shell's primary display font, so Pango's per-character
//! fallback reaches the Nerd Font instead of finding a stray glyph first.
//!
//! # Fallback
//!
//! [`Icon::glyph`] is only used when [`probe_coverage`] has confirmed the
//! resolved font actually has that glyph; otherwise [`Icon::symbolic`] is
//! rendered as a themed icon. The check is per icon rather than per font, so a
//! font missing a single glyph degrades that one control instead of the whole
//! shell, and a missing font degrades to exactly the old behaviour.

use std::cell::RefCell;
use std::collections::HashMap;

use gtk::pango;
use gtk::prelude::*;
use log::info;

use crate::config::IconStyle;

/// Font stack used to resolve glyphs.
///
/// Deliberately short. Fontconfig searches every installed font for a family
/// that covers a requested codepoint, so any patched Nerd Font is found
/// without being named here -- which is what lets this work on an arbitrary
/// machine. `Symbols Nerd Font` leads only because it is the standalone icon
/// font: when it is present every glyph resolves to it, so the icons share one
/// design instead of being sourced from whichever font fontconfig happens to
/// rank first for each individual codepoint.
///
/// Emitted into the stylesheet by [`glyph_font_css`] rather than written in
/// `style.css`, so the font Pango probes and the font GTK draws with cannot
/// disagree.
const ICON_FONT_STACK: &str = "Symbols Nerd Font, Symbols Nerd Font Mono, monospace";

/// The `.icon-glyph` rule, for concatenation into the shell stylesheet.
pub(crate) fn glyph_font_css() -> String {
    format!(
        ".{GLYPH_CLASS} {{\n  \
         font-family: {ICON_FONT_STACK};\n  \
         font-weight: normal;\n  \
         font-style: normal;\n\
         }}\n"
    )
}

/// CSS class carried by every glyph label, so the stylesheet can size and
/// color glyphs the same way it sizes and colors themed icons.
pub(crate) const GLYPH_CLASS: &str = "icon-glyph";

/// A piece of mithshell's own chrome that can be drawn as a glyph.
///
/// Deliberately closed: anything whose icon is chosen at runtime by another
/// program cannot be listed here, because its name is not known until it
/// arrives over D-Bus or IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Icon {
    Close,
    Search,
    Back,
    Forward,
    Refresh,
    Reboot,
    Shutdown,
    Suspend,
    Expand,
    Restore,
    ClearAll,
    Bell,
    BellOff,
    Play,
    Pause,
    Previous,
    Next,
    VolumeMuted,
    VolumeLow,
    VolumeMedium,
    VolumeHigh,
    Brightness,
    Workspace,
    Addon,
    Loading,
    Executable,
    WeatherClear,
    /// Battery at a percentage, rendered as one of five charge steps.
    ///
    /// `charging` only affects the themed fallback, which has real charging
    /// variants. The glyph path signals charging with a CSS class instead;
    /// see [`Icon::glyph`].
    Battery {
        percent: u8,
        charging: bool,
    },
}

/// Every non-parameterised icon, for coverage probing and tests.
const SIMPLE_ICONS: &[Icon] = &[
    Icon::Close,
    Icon::Search,
    Icon::Back,
    Icon::Forward,
    Icon::Refresh,
    Icon::Reboot,
    Icon::Shutdown,
    Icon::Suspend,
    Icon::Expand,
    Icon::Restore,
    Icon::ClearAll,
    Icon::Bell,
    Icon::BellOff,
    Icon::Play,
    Icon::Pause,
    Icon::Previous,
    Icon::Next,
    Icon::VolumeMuted,
    Icon::VolumeLow,
    Icon::VolumeMedium,
    Icon::VolumeHigh,
    Icon::Brightness,
    Icon::Workspace,
    Icon::Addon,
    Icon::Loading,
    Icon::Executable,
    Icon::WeatherClear,
];

/// The five battery charge steps, ordered from empty to full, as
/// `(upper bound inclusive, glyph)`.
const BATTERY_STEPS: [(u8, char); 5] = [
    (10, '\u{f244}'),
    (35, '\u{f243}'),
    (60, '\u{f242}'),
    (85, '\u{f241}'),
    (100, '\u{f240}'),
];

impl Icon {
    /// The Font Awesome codepoint drawn for this icon.
    ///
    /// Font Awesome 4 only distinguishes three speaker levels, so
    /// [`Icon::VolumeLow`] and [`Icon::VolumeMedium`] share a glyph. It also
    /// has no charging-battery icon, so a charging battery keeps its charge
    /// step and is tinted via the `charging` CSS class instead of switching
    /// glyphs. Both fall back to more specific themed icons when glyphs are
    /// unavailable.
    pub(crate) fn glyph(self) -> char {
        match self {
            Self::Close => '\u{f00d}',
            Self::Search => '\u{f002}',
            Self::Back => '\u{f053}',
            Self::Forward => '\u{f054}',
            Self::Refresh | Self::Reboot => '\u{f021}',
            Self::Shutdown => '\u{f011}',
            Self::Suspend => '\u{f186}',
            Self::Expand => '\u{f065}',
            Self::Restore => '\u{f066}',
            Self::ClearAll => '\u{f1f8}',
            Self::Bell => '\u{f0f3}',
            Self::BellOff => '\u{f1f6}',
            Self::Play => '\u{f04b}',
            Self::Pause => '\u{f04c}',
            Self::Previous => '\u{f048}',
            Self::Next => '\u{f051}',
            Self::VolumeMuted => '\u{f026}',
            Self::VolumeLow | Self::VolumeMedium => '\u{f027}',
            Self::VolumeHigh => '\u{f028}',
            Self::Brightness | Self::WeatherClear => '\u{f185}',
            Self::Workspace => '\u{f108}',
            Self::Addon => '\u{f12e}',
            Self::Loading => '\u{f110}',
            Self::Executable => '\u{f15b}',
            Self::Battery { percent, .. } => battery_glyph(percent),
        }
    }

    /// The themed icon name used when [`Icon::glyph`] is unavailable.
    ///
    /// Battery uses the standard freedesktop `battery-level-*` names, which
    /// Adwaita, Papirus and Breeze all provide, rather than the `xsi-*` names
    /// this shell previously depended on.
    pub(crate) fn symbolic(self) -> String {
        match self {
            Self::Battery { percent, charging } => {
                let level = (f64::from(percent.min(100)) / 10.0).round() as u8 * 10;
                let suffix = if charging { "-charging" } else { "" };
                format!("battery-level-{level}{suffix}-symbolic")
            }
            other => other.symbolic_static().to_owned(),
        }
    }

    /// The themed name for every icon whose fallback is a fixed string.
    ///
    /// [`Icon::Battery`] has no single name, so it is mapped to the generic
    /// `battery-symbolic` here and given a level-accurate name by
    /// [`Icon::symbolic`].
    fn symbolic_static(self) -> &'static str {
        match self {
            Self::Close => "window-close-symbolic",
            Self::Search => "system-search-symbolic",
            Self::Back => "go-previous-symbolic",
            Self::Forward => "go-next-symbolic",
            Self::Refresh => "view-refresh-symbolic",
            Self::Reboot => "system-reboot-symbolic",
            Self::Shutdown => "system-shutdown-symbolic",
            Self::Suspend => "system-suspend-symbolic",
            Self::Expand => "view-fullscreen-symbolic",
            Self::Restore => "view-restore-symbolic",
            Self::ClearAll => "edit-clear-all-symbolic",
            Self::Bell => "preferences-system-notifications-symbolic",
            Self::BellOff => "notifications-disabled-symbolic",
            Self::Play => "media-playback-start-symbolic",
            Self::Pause => "media-playback-pause-symbolic",
            Self::Previous => "media-skip-backward-symbolic",
            Self::Next => "media-skip-forward-symbolic",
            Self::VolumeMuted => "audio-volume-muted-symbolic",
            Self::VolumeLow => "audio-volume-low-symbolic",
            Self::VolumeMedium => "audio-volume-medium-symbolic",
            Self::VolumeHigh => "audio-volume-high-symbolic",
            Self::Brightness => "display-brightness-symbolic",
            Self::Workspace => "focus-windows-symbolic",
            Self::Addon => "application-x-addon-symbolic",
            Self::Loading => "content-loading-symbolic",
            Self::Executable => "application-x-executable-symbolic",
            Self::WeatherClear => "weather-clear-symbolic",
            Self::Battery { .. } => "battery-symbolic",
        }
    }

    /// A human-readable label, used as the accessible name.
    ///
    /// A glyph label otherwise exposes a private-use codepoint to assistive
    /// technology, which a themed `gtk::Image` would have described for us.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Search => "Search",
            Self::Back => "Back",
            Self::Forward => "Forward",
            Self::Refresh => "Reload",
            Self::Reboot => "Restart",
            Self::Shutdown => "Shut down",
            Self::Suspend => "Suspend",
            Self::Expand => "Expand",
            Self::Restore => "Restore",
            Self::ClearAll => "Clear all",
            Self::Bell => "Notifications",
            Self::BellOff => "Notifications silenced",
            Self::Play => "Play",
            Self::Pause => "Pause",
            Self::Previous => "Previous track",
            Self::Next => "Next track",
            Self::VolumeMuted => "Muted",
            Self::VolumeLow => "Volume low",
            Self::VolumeMedium => "Volume medium",
            Self::VolumeHigh => "Volume high",
            Self::Brightness => "Brightness",
            Self::Workspace => "Workspace",
            Self::Addon => "Plugin",
            Self::Loading => "Loading",
            Self::Executable => "Application",
            Self::WeatherClear => "Weather",
            Self::Battery { .. } => "Battery",
        }
    }

    /// Groups every `Battery` variant onto one probe entry.
    ///
    /// Coverage is cached per key rather than per value so that battery level
    /// changes do not grow the cache without bound.
    fn probe_key(self) -> Icon {
        match self {
            Self::Battery { .. } => Self::Battery {
                percent: 100,
                charging: false,
            },
            other => other,
        }
    }
}

/// Maps a charge percentage onto one of the five Font Awesome battery glyphs.
fn battery_glyph(percent: u8) -> char {
    let percent = percent.min(100);
    BATTERY_STEPS
        .iter()
        .find(|(upper, _)| percent <= *upper)
        .map_or(BATTERY_STEPS[BATTERY_STEPS.len() - 1].1, |(_, glyph)| {
            *glyph
        })
}

thread_local! {
    /// Per-icon glyph availability, populated once by [`probe_coverage`].
    ///
    /// GTK is single-threaded and every caller runs on the main loop, so a
    /// thread-local is sufficient and avoids a lock on a hot path.
    static COVERAGE: RefCell<Option<HashMap<Icon, bool>>> = const { RefCell::new(None) };
}

/// Resolves whether each icon's glyph can actually be drawn, caching the
/// result for the rest of the process.
///
/// Must be called after GTK is initialised, since it needs a Pango context.
/// Calling it more than once is harmless; later calls are ignored so the
/// answer stays stable even if fontconfig changes underneath us, which would
/// otherwise let widgets built at different times disagree.
pub(crate) fn probe_coverage() {
    COVERAGE.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }

        // A throwaway widget is the supported way to reach a Pango context
        // for the default display without a realized window.
        let context = gtk::Label::new(None).pango_context();
        let description = pango::FontDescription::from_string(ICON_FONT_STACK);
        let fontset = context.load_fontset(&description, &pango::Language::default());

        let mut coverage = HashMap::new();
        let mut missing = Vec::new();
        for icon in SIMPLE_ICONS
            .iter()
            .copied()
            .chain(std::iter::once(Icon::Battery {
                percent: 100,
                charging: false,
            }))
        {
            let glyph = icon.glyph();
            let covered = fontset
                .as_ref()
                .map(|fontset| fontset.font(glyph as u32).has_char(glyph))
                .unwrap_or(false);
            if !covered {
                missing.push(icon);
            }
            coverage.insert(icon, covered);
        }

        if missing.is_empty() {
            info!(
                "icon glyphs available for all {} chrome icons",
                coverage.len()
            );
        } else {
            info!(
                "{} of {} icon glyphs unavailable in \"{ICON_FONT_STACK}\", \
                 falling back to themed icons for: {}",
                missing.len(),
                coverage.len(),
                missing
                    .iter()
                    .map(|icon| icon.label())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }

        *cell.borrow_mut() = Some(coverage);
    });
}

/// Whether `icon` should be drawn as a glyph under the configured style.
///
/// Returns `false` before [`probe_coverage`] has run, so an icon built too
/// early degrades to a themed image rather than risking an unrenderable glyph.
fn use_glyph(icon: Icon, style: IconStyle) -> bool {
    if style == IconStyle::Symbolic {
        return false;
    }
    COVERAGE.with(|cell| {
        cell.borrow()
            .as_ref()
            .and_then(|coverage| coverage.get(&icon.probe_key()).copied())
            .unwrap_or(false)
    })
}

/// Builds the widget for `icon`: a glyph label when possible, else a themed
/// image.
///
/// The returned widget is always the one to append; callers keep it as a
/// `gtk::Widget` and mutate it later through [`set_icon`], which handles both
/// representations.
pub(crate) fn icon_widget(icon: Icon, style: IconStyle) -> gtk::Widget {
    if use_glyph(icon, style) {
        let label = gtk::Label::new(Some(&icon.glyph().to_string()));
        label.add_css_class(GLYPH_CLASS);
        apply_battery_state(label.upcast_ref(), icon);
        label.update_property(&[gtk::accessible::Property::Label(icon.label())]);
        label.upcast()
    } else {
        let image = gtk::Image::from_icon_name(&icon.symbolic());
        apply_battery_state(image.upcast_ref(), icon);
        image.update_property(&[gtk::accessible::Property::Label(icon.label())]);
        image.upcast()
    }
}

/// Retargets a widget built by [`icon_widget`] at a different icon.
///
/// Only meaningful when the new icon uses the same representation as the old
/// one, which holds for every runtime transition in the shell: the icons that
/// change (volume level, play/pause, expand/restore, battery) either all have
/// glyphs or all fall back together, because coverage is resolved once for the
/// whole set.
pub(crate) fn set_icon(widget: &gtk::Widget, icon: Icon, style: IconStyle) {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        label.set_label(&icon.glyph().to_string());
    } else if let Some(image) = widget.downcast_ref::<gtk::Image>() {
        image.set_icon_name(Some(&icon.symbolic()));
    }
    let _ = style;
    apply_battery_state(widget, icon);
    widget.update_property(&[gtk::accessible::Property::Label(icon.label())]);
}

/// Marks a charging battery so the stylesheet can tint it.
///
/// Font Awesome has no charging-battery glyph, so charge state is carried by a
/// CSS class instead of a different codepoint. Applied to the themed path too
/// so both representations respond to the same rule.
fn apply_battery_state(widget: &gtk::Widget, icon: Icon) {
    let charging = matches!(icon, Icon::Battery { charging: true, .. });
    if charging {
        widget.add_css_class("charging");
    } else {
        widget.remove_css_class("charging");
    }
}

/// Builds a button whose only content is `icon`.
///
/// Replaces `gtk::Button::from_icon_name`, which would force the themed
/// representation and nest a `image` node that the stylesheet would have to
/// target separately.
pub(crate) fn icon_button(icon: Icon, style: IconStyle) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_child(Some(&icon_widget(icon, style)));
    button.update_property(&[gtk::accessible::Property::Label(icon.label())]);
    button
}

/// Retargets a button built by [`icon_button`].
pub(crate) fn set_button_icon(button: &impl IsA<gtk::Button>, icon: Icon, style: IconStyle) {
    let button = button.as_ref();
    match button.child() {
        Some(child) => set_icon(&child, icon, style),
        None => button.set_child(Some(&icon_widget(icon, style))),
    }
    button.update_property(&[gtk::accessible::Property::Label(icon.label())]);
}

/// Builds an icon widget for a name supplied by another program.
///
/// Absolute paths are loaded from disk, non-empty names go through the icon
/// theme, and anything else falls back to `fallback`. Shared by notifications,
/// the tray and the TarraGon launcher, which all receive icons this way and
/// previously each reimplemented the same path-versus-name test.
pub(crate) fn foreign_image(name: Option<&str>, fallback: Icon) -> gtk::Image {
    let image = gtk::Image::new();
    set_foreign_image(&image, name, fallback);
    image
}

/// Points an existing `gtk::Image` at a foreign icon name or path.
pub(crate) fn set_foreign_image(image: &gtk::Image, name: Option<&str>, fallback: Icon) {
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(path) if path.starts_with('/') => image.set_from_file(Some(path)),
        Some(name) => image.set_icon_name(Some(name)),
        None => image.set_icon_name(Some(&fallback.symbolic())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_in_the_stable_font_awesome_range() {
        // Nerd Fonts v3 moved the Material Design Icons block but left Font
        // Awesome alone, so staying inside this range is what lets one table
        // serve both font generations.
        for icon in SIMPLE_ICONS {
            let glyph = icon.glyph() as u32;
            assert!(
                (0xf000..=0xf2ff).contains(&glyph),
                "{icon:?} uses U+{glyph:04X}, outside the stable Font Awesome range",
            );
        }
        for (_, glyph) in BATTERY_STEPS {
            let glyph = glyph as u32;
            assert!(
                (0xf000..=0xf2ff).contains(&glyph),
                "battery step uses U+{glyph:04X}, outside the stable Font Awesome range",
            );
        }
    }

    #[test]
    fn battery_glyph_covers_the_full_range_in_five_steps() {
        let steps: Vec<char> = (0..=100).step_by(5).map(battery_glyph).collect();
        assert_eq!(steps.first(), Some(&'\u{f244}'));
        assert_eq!(steps.last(), Some(&'\u{f240}'));
        // Monotonic: charge never appears to drop as the percentage rises.
        let mut seen = Vec::new();
        for step in steps {
            if seen.last() != Some(&step) {
                seen.push(step);
            }
        }
        assert_eq!(
            seen,
            vec!['\u{f244}', '\u{f243}', '\u{f242}', '\u{f241}', '\u{f240}'],
        );
    }

    #[test]
    fn battery_glyph_saturates_above_full() {
        assert_eq!(battery_glyph(101), '\u{f240}');
        assert_eq!(battery_glyph(255), '\u{f240}');
    }

    #[test]
    fn battery_symbolic_uses_standard_freedesktop_names() {
        // `xsi-*` needs xapp-symbolic-icons installed; these ship with Adwaita,
        // Papirus and Breeze alike.
        let icon = Icon::Battery {
            percent: 44,
            charging: false,
        };
        assert_eq!(icon.symbolic(), "battery-level-40-symbolic");
        let icon = Icon::Battery {
            percent: 96,
            charging: true,
        };
        assert_eq!(icon.symbolic(), "battery-level-100-charging-symbolic");
    }

    #[test]
    fn battery_symbolic_clamps_out_of_range_percentages() {
        let icon = Icon::Battery {
            percent: 200,
            charging: false,
        };
        assert_eq!(icon.symbolic(), "battery-level-100-symbolic");
    }

    #[test]
    fn every_battery_level_maps_to_a_documented_name() {
        for percent in 0..=100u8 {
            let name = Icon::Battery {
                percent,
                charging: false,
            }
            .symbolic();
            let level: u8 = name
                .trim_start_matches("battery-level-")
                .trim_end_matches("-symbolic")
                .parse()
                .expect("level should parse");
            assert_eq!(level % 10, 0, "{name} is not a decile");
            assert!(level <= 100, "{name} exceeds 100");
        }
    }

    #[test]
    fn simple_icons_have_distinct_symbolic_names() {
        let mut names: Vec<&str> = SIMPLE_ICONS.iter().map(|i| i.symbolic_static()).collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(count, names.len(), "duplicate themed icon names");
    }

    #[test]
    fn battery_variants_share_one_probe_key() {
        let a = Icon::Battery {
            percent: 5,
            charging: true,
        };
        let b = Icon::Battery {
            percent: 90,
            charging: false,
        };
        assert_eq!(a.probe_key(), b.probe_key());
        assert_eq!(Icon::Close.probe_key(), Icon::Close);
    }

    #[test]
    fn glyphs_are_absent_until_coverage_is_probed() {
        // Widgets built before GTK starts must not gamble on a glyph.
        assert!(!use_glyph(Icon::Close, IconStyle::Glyph));
    }

    #[test]
    fn symbolic_style_never_uses_glyphs() {
        assert!(!use_glyph(Icon::Close, IconStyle::Symbolic));
    }
}
