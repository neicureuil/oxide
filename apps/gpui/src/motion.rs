//! The app's few animation curves and transitions (`Motion.swift`), all dropped when macOS asks to reduce motion.
//!
//! GPUI plays an animation when an element appears (it was not drawn in the previous frame) or when its
//! id changes. So "appear" transitions use a fixed id, and a change of state puts the state in the id.
use gpui::{
    Animation, AnimationExt, AnyElement, Div, ElementId, Hsla, IntoElement, ParentElement, Pixels,
    SharedString, Stateful, StatefulInteractiveElement, Styled, div, ease_in_out, px,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Duration;

thread_local! {
    static REDUCED: Cell<Option<bool>> = const { Cell::new(None) };
    /// Hover state per key: true while hovered; absent until first hovered (nothing to fade out yet).
    static HOVERED: RefCell<HashMap<SharedString, bool>> = RefCell::default();
    /// Where each item of a sliding selection was last laid out: (left, width), from the first item's left.
    static SLOTS: RefCell<HashMap<String, Vec<(f32, f32)>>> = RefCell::default();
    /// When each key's value last changed, for elements of virtualised lists that redraw on scroll.
    static CHANGED: RefCell<HashMap<String, (f32, std::time::Instant)>> = RefCell::default();
    /// Last value and the one before, per key, for transitions that move from the old value to the new.
    static VALUES: RefCell<HashMap<String, (f32, f32)>> = RefCell::default();
}

/// macOS › Accessibility › Display › Reduce motion; read again when the window becomes active.
pub fn reduced() -> bool {
    REDUCED.with(|r| {
        r.get().unwrap_or_else(|| {
            let value = read_reduced();
            r.set(Some(value));
            value
        })
    })
}
pub fn refresh_reduced() {
    REDUCED.with(|r| r.set(Some(read_reduced())));
}
#[cfg(not(target_os = "macos"))]
fn read_reduced() -> bool {
    false
}
#[cfg(target_os = "macos")]
fn read_reduced() -> bool {
    let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
    // Not bound by objc2-app-kit's NSWorkspace feature set: ask the runtime directly.
    unsafe { objc2::msg_send![&*workspace, accessibilityDisplayShouldReduceMotion] }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// Hover and press feedback: quick enough not to lag the pointer (0.12 s ease-out).
    Hover,
    /// A selection moving between items: rail, tabs, chips (spring, 0.3 s, slightly damped).
    Select,
    /// Screens, panels and banners coming and going (0.2 s ease-in-out).
    Standard,
    /// Badges and dots popping in (springy, 0.25 s).
    Pop,
}

impl Curve {
    fn duration(self) -> Duration {
        Duration::from_millis(match self {
            Curve::Hover => 120,
            Curve::Select => 300,
            Curve::Standard => 200,
            Curve::Pop => 250,
        })
    }
    /// Progress along the curve; springs overshoot 1 on purpose, which GPUI's own easing forbids
    /// (it asserts 0…1), so GPUI runs linear time and this shapes it.
    fn shape(self, t: f32) -> f32 {
        match self {
            Curve::Hover => ease_out(t),
            Curve::Select => spring(t, 0.85),
            Curve::Standard => ease_in_out(t),
            Curve::Pop => spring(t, 0.6),
        }
    }
    fn animation(self) -> Animation {
        Animation::new(self.duration())
    }
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

/// A critically damped-ish spring settling at 1 by the end of the animation; lower damping overshoots more.
pub fn spring(t: f32, damping: f32) -> f32 {
    if t >= 1. {
        return 1.;
    }
    // A decaying cosine scaled so the motion is over at t = 1.
    let zeta = damping.clamp(0.1, 1.);
    let omega = 4.5 * std::f32::consts::PI * (1. - zeta * 0.6);
    let decay = (-zeta * 7. * t).exp();
    1. - decay * (omega * t).cos() * (1. - t)
}

/// Runs `animator` from 0 to 1 when the element appears or `id` changes; at rest (or motion reduced) it is drawn at 1.
pub fn animate<E: IntoElement + 'static>(
    element: E,
    id: impl Into<ElementId>,
    curve: Curve,
    animator: impl Fn(E, f32) -> E + 'static,
) -> AnyElement {
    if reduced() {
        return animator(element, 1.).into_any_element();
    }
    element
        .with_animation(id, curve.animation(), move |e, t| animator(e, curve.shape(t)))
        .into_any_element()
}

/// Fades in.
pub fn fade_in<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> AnyElement {
    animate(element, id, Curve::Standard, |e, t| e.opacity(t))
}

/// Fades in after a short wait, so fast loads do not flash a spinner (`LoadingLabel`).
pub fn fade_in_late<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> AnyElement {
    if reduced() {
        return element.into_any_element();
    }
    element
        .with_animation(
            id,
            Animation::new(Duration::from_millis(320)).with_easing(|t| ((t - 0.375) / 0.625).clamp(0., 1.)),
            |e, t| e.opacity(t),
        )
        .into_any_element()
}

/// Banners and bars sliding down from their edge (`slideDown`).
pub fn slide_down<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> AnyElement {
    animate(element, id, Curve::Standard, |e, t| e.opacity(t).mt(px(-10. * (1. - t))))
}

/// Floating pills rising from the bottom (`rise`).
pub fn rise<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> AnyElement {
    animate(element, id, Curve::Standard, |e, t| e.opacity(t).mt(px(12. * (1. - t))))
}

/// A panel coming in from the right edge (`move(edge: .trailing)`).
pub fn slide_from_trailing<E: IntoElement + Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
) -> AnyElement {
    animate(element, id, Curve::Standard, |e, t| e.opacity(t).ml(px(36. * (1. - t))))
}

/// Badges and unread dots popping in (`pop`): fade with a small springy drop into place.
pub fn pop<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> AnyElement {
    animate(element, id, Curve::Pop, |e, t| {
        e.opacity(t.clamp(0., 1.)).mt(px(-4. * (1. - t)))
    })
}

/// Menus and popovers: a quick fade with a short drop, as AppKit menus open.
pub fn menu_in<E: IntoElement + Styled + 'static>(element: E, id: impl Into<ElementId>) -> AnyElement {
    animate(element, id, Curve::Hover, |e, t| e.opacity(t).mt(px(-4. * (1. - t))))
}

/// Hover feedback that eases in and out (`Motion.hover`) instead of GPUI's instant `.hover()` style:
/// a `color` layer behind the content fades with the pointer. `key` must be unique on screen.
pub fn hover_fill(element: Stateful<Div>, key: impl Into<SharedString>, color: Hsla, radius: Pixels) -> Stateful<Div> {
    let key: SharedString = key.into();
    let state = HOVERED.with(|h| h.borrow().get(&key).copied());
    let layer = div().absolute().inset_0().rounded(radius).bg(color);
    let layer = match state {
        // Never hovered: nothing to draw, and nothing to fade out on first appearance.
        None => None,
        Some(on) if reduced() => on.then(|| layer.into_any_element()),
        Some(on) => Some(
            layer
                .with_animation(
                    ElementId::Name(format!("{key}-hover-{on}").into()),
                    Curve::Hover.animation(),
                    move |d, t| {
                        let t = Curve::Hover.shape(t);
                        d.opacity(if on { t } else { 1. - t })
                    },
                )
                .into_any_element(),
        ),
    };
    let hovered_key = key.clone();
    element
        .relative()
        .children(layer)
        .on_hover(move |hovered, window, _| {
            HOVERED.with(|h| h.borrow_mut().insert(hovered_key.clone(), *hovered));
            window.refresh();
        })
}

/// A selection that slides between items of a row (`matchedGeometryEffect`): chips, tabs, segments.
///
/// Put the returned highlight first in the row (it is absolutely placed, `lead` points from the row's left
/// edge to its first item), and the returned listener on the row's `on_children_prepainted`, which tells
/// where the items are. `shape` gives the highlight its look and vertical place. Items are measured
/// after a frame, so for that first frame (or when nothing is selected) there is no highlight: items
/// should draw their own selected background when `slid` is false.
pub fn sliding_selection(
    key: &str,
    selected: Option<usize>,
    lead: f32,
    shape: impl Fn(Div) -> Div + 'static,
) -> (Option<AnyElement>, bool, impl Fn(Vec<gpui::Bounds<Pixels>>, &mut gpui::Window, &mut gpui::App) + 'static) {
    let slots = SLOTS.with(|s| s.borrow().get(key).cloned()).unwrap_or_default();
    let store_key = key.to_owned();
    let listener = move |bounds: Vec<gpui::Bounds<Pixels>>, window: &mut gpui::Window, _: &mut gpui::App| {
        // The highlight is the first child: items follow it.
        let items: Vec<_> = bounds.iter().skip(1).collect();
        let Some(first) = items.first() else { return };
        let origin = f32::from(first.origin.x);
        let measured: Vec<(f32, f32)> = items
            .iter()
            .map(|b| (f32::from(b.origin.x) - origin, f32::from(b.size.width)))
            .collect();
        let changed = SLOTS.with(|s| {
            let mut s = s.borrow_mut();
            let changed = s.get(&store_key) != Some(&measured);
            s.insert(store_key.clone(), measured);
            changed
        });
        if changed {
            window.refresh();
        }
    };
    let Some(ix) = selected else {
        return (None, false, listener);
    };
    let Some(&(to_x, to_w)) = slots.get(ix) else {
        return (None, false, listener);
    };
    let from_ix = previous(&format!("{key}-selected"), ix as f32) as usize;
    let (from_x, from_w) = slots.get(from_ix).copied().unwrap_or((to_x, to_w));
    let highlight = animate(
        shape(div().absolute()),
        move_id(key, from_ix as f32, ix as f32),
        Curve::Select,
        move |d, t| d.left(px(lead + lerp(from_x, to_x, t))).w(px(lerp(from_w, to_w, t))),
    );
    (Some(highlight), true, listener)
}

/// True within a moment of `key`'s value changing (not on the first sighting): list rows scrolled back
/// into view must not replay a pop that belongs to the change.
pub fn just_changed(key: &str, value: f32) -> bool {
    CHANGED.with(|c| {
        let mut c = c.borrow_mut();
        match c.get(key) {
            None => {
                c.insert(key.to_owned(), (value, std::time::Instant::now() - Duration::from_secs(60)));
                false
            }
            Some((old, _)) if *old != value => {
                c.insert(key.to_owned(), (value, std::time::Instant::now()));
                true
            }
            Some((_, at)) => at.elapsed() < Duration::from_millis(400),
        }
    })
}

/// The value `key` had before its latest change, to animate from it; `value` becomes the latest.
pub fn previous(key: &str, value: f32) -> f32 {
    VALUES.with(|values| {
        let mut values = values.borrow_mut();
        let entry = values.entry(key.to_owned()).or_insert((value, value));
        if entry.0 != value {
            *entry = (value, entry.0);
        }
        entry.1
    })
}

/// An id that changes with the move, so the transition plays once per change.
pub fn move_id(key: &str, from: f32, to: f32) -> ElementId {
    ElementId::Name(format!("{key}:{from}>{to}").into())
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A light band sweeping across one placeholder shape, Teams-style (`shimmer`); none when motion is reduced.
/// Shapes of one skeleton appear in the same frame, so their bands move as one across `sweep` points;
/// `x` is the shape's left edge within that sweep. The band shows only on the shape, as Swift's mask.
pub fn shimmer_shape(shape: Div, id: impl Into<ElementId>, x: f32, sweep: f32, tint: Hsla) -> Div {
    if reduced() {
        return shape;
    }
    let band = sweep * 0.6;
    let light = div()
        .absolute()
        .top_0()
        .bottom_0()
        .w(px(band))
        .bg(gpui::linear_gradient(
            90.,
            gpui::linear_color_stop(tint.opacity(0.), 0.),
            gpui::linear_color_stop(tint, 1.),
        ))
        .with_animation(
            id,
            Animation::new(Duration::from_millis(1300)).repeat(),
            move |e, t| e.left(px(-band + t * (sweep + band * 1.6) - x)),
        );
    shape.relative().overflow_hidden().child(light)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn springs_end_at_rest_and_pop_overshoots() {
        assert_eq!(spring(1., 0.6), 1.);
        assert!(spring(0., 0.85).abs() < 1e-6);
        let peak = (0..100).map(|i| spring(i as f32 / 100., 0.6)).fold(0., f32::max);
        assert!(peak > 1.0, "pop overshoots, got {peak}");
        let select = (0..100).map(|i| spring(i as f32 / 100., 0.85)).fold(0., f32::max);
        assert!(select < peak);
    }

    #[test]
    fn previous_tracks_the_value_before_the_latest_change() {
        assert_eq!(previous("rail", 1.), 1.);
        assert_eq!(previous("rail", 1.), 1.);
        assert_eq!(previous("rail", 3.), 1.);
        // Stays on the same move until the value changes again.
        assert_eq!(previous("rail", 3.), 1.);
        assert_eq!(previous("rail", 0.), 3.);
    }
}
