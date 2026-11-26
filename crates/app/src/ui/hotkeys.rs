//! Hot keys: every function and slider the keyboard can reach, with the keys it has out
//! of the box, and the keys the user has given it instead.
//!
//! [`commands`] lists them; [`Keymap`] holds the user's changes and each slider's step
//! (saved with the app's settings, not the catalog). The dispatcher in
//! [`super::shortcuts`] looks keys up through [`current`], the keymap in use.
//!
//! A slider's keys move it by its step. Sliders that are plain settings are moved where
//! the settings are (see [`moves_settings`]); the others when their row is drawn
//! ([`drawn`]), which also keeps the order `,` and `.` pick sliders in.

use std::collections::BTreeMap;
use std::sync::{LazyLock, OnceLock, RwLock};

use eframe::egui::{self, Key};
use imaging::develop::{
    COLORS, DetailParam, EffectParam, GradeParam, GradeWheel, HslGroup, LocalParam, Slider, TransformParam,
};
use serde::{Deserialize, Serialize};

use super::shortcuts::{Action, Binding, SHORTCUTS, Scope};

/// What a command does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Run {
    Action(Action),
    /// Move a slider by its step, up or down.
    Slider {
        slider: Slider,
        up: bool,
    },
    /// Pick the previous (−1) or next (1) slider in the panel shown.
    PickSlider(i8),
    /// Move the picked slider by its step.
    StepPicked(i8),
    /// Press a panel's button, flip its checkbox or pick its choice (when it is drawn;
    /// see [`Hot`]).
    Control,
    /// Something the app does itself (a menu item, the Library's sort and filter): see
    /// [`super::app_keys`].
    App,
}

/// Something the keyboard can do.
pub struct Command {
    /// Stable: the user's keys are saved under it.
    pub id: String,
    pub name: String,
    /// Where it is listed.
    pub section: &'static str,
    /// The Develop panel it is in (opened when its key is pressed).
    pub panel: Option<&'static str>,
    pub scope: Scope,
    pub run: Run,
    /// The keys it has out of the box.
    pub defaults: Vec<Binding>,
}

impl Command {
    /// The action it runs, if it is one.
    #[cfg(test)]
    pub fn action(&self) -> Option<Action> {
        match self.run {
            Run::Action(a) => Some(a),
            _ => None,
        }
    }

    /// The slider it moves, if it does.
    pub fn slider(&self) -> Option<Slider> {
        match self.run {
            Run::Slider { slider, .. } => Some(slider),
            _ => None,
        }
    }
}

/// An action's id: its name and values, as written in the code ("SetRating.stars=3.advance=false").
fn action_id(action: Action) -> String {
    format!("{action:?}").replace(" { ", ".").replace(" }", "").replace(": ", "=").replace(", ", ".")
}

/// A slider's id (the same for its Up and Down, and for its step).
pub fn slider_id(slider: Slider) -> String {
    format!("slider.{:?}", slider_key(slider)).replace(' ', "")
}

/// The slider as hot keys know it: a mask's slider is the shown mask's, whichever it is.
pub fn slider_key(slider: Slider) -> Slider {
    match slider {
        Slider::Local(_, p) => Slider::Local(0, p),
        Slider::MaskAmount(i) if i < STAND_IN => Slider::MaskAmount(0),
        Slider::MaskFeather(..) => Slider::MaskFeather(0, 0),
        Slider::MaskDepthNear(..) => Slider::MaskDepthNear(0, 0),
        Slider::MaskDepthFar(..) => Slider::MaskDepthFar(0, 0),
        s => s,
    }
}

/// From here on, `Slider::MaskAmount(n)` stands in for a slider that is not a develop
/// setting (Lens, Remove, the brush): `n` is that slider.
const STAND_IN: u8 = 200;

/// "VignetteAmount" → "Vignette Amount".
fn words(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

const BASIC: &str = "Develop ▸ Basic";
const CURVE: &str = "Develop ▸ Tone Curve";
const HSL: &str = "Develop ▸ HSL / Color";
const GRADING: &str = "Develop ▸ Color Grading";
const DETAIL: &str = "Develop ▸ Detail";
const LENS: &str = "Develop ▸ Lens Corrections";
const TRANSFORM: &str = "Develop ▸ Transform";
const EFFECTS: &str = "Develop ▸ Effects";
const MASKING: &str = "Develop ▸ Masking";
const REMOVE: &str = "Develop ▸ Remove";
const CROP: &str = "Develop ▸ Crop";
const SLIDERS: &str = "Develop ▸ Sliders";

/// Every slider: what it is, its name, its section and its Develop panel.
fn sliders() -> Vec<(Slider, String, &'static str, Option<&'static str>)> {
    let mut out = Vec::new();
    let mut add = |s: Slider, name: String, section: &'static str, panel: Option<&'static str>| {
        out.push((s, name, section, panel));
    };
    let basic = Some("Basic");
    for s in Slider::WB.into_iter().chain(Slider::TONE).chain(Slider::PRESENCE) {
        add(s, s.label().to_owned(), BASIC, basic);
    }
    add(Slider::MaskAmount(254), "Profile Amount".into(), BASIC, basic);
    for s in Slider::CURVE {
        add(s, format!("Curve {}", s.label()), CURVE, Some("Tone Curve"));
    }
    for g in HslGroup::ALL {
        for i in 0..COLORS.len() as u8 {
            add(Slider::Hsl(g, i), format!("{g:?} {}", COLORS[usize::from(i)]), HSL, Some("HSL / Color"));
        }
    }
    for i in 0..COLORS.len() as u8 {
        add(Slider::BwMix(i), format!("B&W {}", COLORS[usize::from(i)]), HSL, Some("HSL / Color"));
    }
    for w in GradeWheel::ALL {
        for p in [GradeParam::Hue, GradeParam::Saturation, GradeParam::Luminance] {
            add(Slider::Grade(w, p), format!("{w:?} {}", p.label()), GRADING, Some("Color Grading"));
        }
    }
    add(Slider::GradeBlending, "Blending".into(), GRADING, Some("Color Grading"));
    add(Slider::GradeBalance, "Balance".into(), GRADING, Some("Color Grading"));
    for p in DetailParam::ALL {
        add(Slider::Detail(p), words(&format!("{p:?}")), DETAIL, Some("Detail"));
    }
    let lens = Some("Lens Corrections");
    for (id, name) in [
        (200, "Profile Distortion"),
        (201, "Profile Vignetting"),
        (202, "Distortion"),
        (203, "Purple Amount"),
        (213, "Purple Hue From"),
        (223, "Purple Hue To"),
        (204, "Green Amount"),
        (214, "Green Hue From"),
        (224, "Green Hue To"),
        (205, "Vignetting Amount"),
        (206, "Vignetting Midpoint"),
    ] {
        add(Slider::MaskAmount(id), name.into(), LENS, lens);
    }
    for p in TransformParam::ALL {
        add(Slider::Transform(p), p.label().to_owned(), TRANSFORM, Some("Transform"));
    }
    for p in EffectParam::ALL {
        add(Slider::Effect(p), words(&format!("{p:?}")), EFFECTS, Some("Effects"));
    }
    for p in LocalParam::ALL {
        add(Slider::Local(0, p), format!("Mask {}", p.label()), MASKING, None);
    }
    add(Slider::MaskAmount(0), "Mask Amount".into(), MASKING, None);
    add(Slider::MaskFeather(0, 0), "Mask Feather".into(), MASKING, None);
    add(Slider::MaskDepthNear(0, 0), "Depth Near".into(), MASKING, None);
    add(Slider::MaskDepthFar(0, 0), "Depth Far".into(), MASKING, None);
    for (id, name) in
        [(250, "Brush Size"), (251, "Brush Feather"), (252, "Brush Flow"), (253, "Brush Density")]
    {
        add(Slider::MaskAmount(id), name.into(), MASKING, None);
    }
    for (id, name) in [
        (240, "Spot Size"),
        (241, "Spot Feather"),
        (242, "Spot Opacity"),
        (243, "Dust Sensitivity"),
        (244, "Lines Sensitivity"),
        (245, "Lines Max Size"),
    ] {
        add(Slider::MaskAmount(id), name.into(), REMOVE, None);
    }
    add(Slider::CropAngle, "Crop Angle".into(), CROP, None);
    out
}

/// Every panel control: its id (as the panel's code names it), name, section and panel.
fn controls() -> Vec<(String, String, &'static str, Option<&'static str>)> {
    use imaging::develop::{CurveChannel, POINT_PRESETS, Upright, VignetteStyle, WbPreset};
    let mut out = Vec::new();
    let mut add = |id: String, name: String, section: &'static str, panel: Option<&'static str>| {
        out.push((format!("develop.{id}"), name, section, panel));
    };
    let basic = Some("Basic");
    add("basic.treatment.color".into(), "Treatment: Color".into(), BASIC, basic);
    add("basic.treatment.bw".into(), "Treatment: Black & White".into(), BASIC, basic);
    add("basic.profile.none".into(), "Profile: None".into(), BASIC, basic);
    add("basic.profile.import".into(), "Import LUT…".into(), BASIC, basic);
    for p in WbPreset::RAW {
        add(format!("basic.wb.{p:?}"), format!("White Balance: {}", p.label()), BASIC, basic);
    }
    let curve = Some("Tone Curve");
    add("curve.parametric".into(), "Parametric Curve".into(), CURVE, curve);
    add("curve.point".into(), "Point Curve".into(), CURVE, curve);
    for c in CurveChannel::ALL {
        add(format!("curve.channel.{c:?}"), format!("Point Curve Channel: {}", c.label()), CURVE, curve);
    }
    for (name, _) in POINT_PRESETS {
        add(format!("curve.preset.{name}"), format!("Point Curve: {name}"), CURVE, curve);
    }
    add("curve.reset.channel".into(), "Reset the Channel's Point Curve".into(), CURVE, curve);
    add("curve.reset.points".into(), "Reset Point Curves".into(), CURVE, curve);
    add("curve.reset.parametric".into(), "Reset Parametric Curve".into(), CURVE, curve);
    let hsl = Some("HSL / Color");
    for (id, name) in
        [("hue", "Hue"), ("saturation", "Saturation"), ("luminance", "Luminance"), ("all", "All")]
    {
        add(format!("hsl.tab.{id}"), format!("HSL: Show {name}"), HSL, hsl);
    }
    let grading = Some("Color Grading");
    for (id, name) in [
        ("all", "3-Way"),
        ("shadows", "Shadows"),
        ("midtones", "Midtones"),
        ("highlights", "Highlights"),
        ("global", "Global"),
    ] {
        add(format!("grading.view.{id}"), format!("Color Grading: Show {name}"), GRADING, grading);
    }
    let detail = Some("Detail");
    add("detail.picker".into(), "Detail Preview: Pick a Point".into(), DETAIL, detail);
    add("detail.preview".into(), "Detail Preview: Show / Hide".into(), DETAIL, detail);
    let transform = Some("Transform");
    for m in Upright::ALL {
        add(format!("transform.upright.{m:?}"), format!("Upright: {}", m.label()), TRANSFORM, transform);
    }
    add("transform.clear_guides".into(), "Clear Guides".into(), TRANSFORM, transform);
    add("transform.constrain".into(), "Transform: Constrain Crop".into(), TRANSFORM, transform);
    let effects = Some("Effects");
    for s in VignetteStyle::ALL {
        add(format!("effects.vignette.{s:?}"), format!("Vignette Style: {}", s.label()), EFFECTS, effects);
    }
    add("effects.depth.show".into(), "Show Depth".into(), EFFECTS, effects);
    add("effects.depth.surface".into(), "Show Surface".into(), EFFECTS, effects);
    add("effects.depth.remake".into(), "Remake the Depth Map".into(), EFFECTS, effects);
    add("effects.depth.make".into(), "Make the Depth Map".into(), EFFECTS, effects);
    add("effects.blur.picker".into(), "Lens Blur: Focus Selector".into(), EFFECTS, effects);
    add("effects.relight.picker".into(), "Relight: Light Selector".into(), EFFECTS, effects);
    add("effects.relight.subject".into(), "Relight: Light the Subject".into(), EFFECTS, effects);
    add("effects.relight.advanced".into(), "Relight: Advanced Settings".into(), EFFECTS, effects);
    add("effects.surface.make".into(), "Make the Surface Map".into(), EFFECTS, effects);
    add("tools.edit".into(), "Edit: Close the Tool (Crop, Remove, Masking)".into(), "Develop", None);
    for (id, name) in [
        ("visible", "Show / Hide the Mask"),
        ("gpu", "Masks: Use GPU for AI"),
        ("depth", "Masks: Refine with Depth"),
        ("rename", "Rename the Mask…"),
        ("duplicate", "Duplicate the Mask"),
        ("delete", "Delete the Mask"),
        ("reset", "Reset the Mask's Sliders"),
        ("component.invert", "Invert the Mask Part"),
        ("component.delete", "Delete the Mask Part"),
        ("brush.erase", "Brush: Erase"),
        ("color.none", "Mask Color: None"),
        ("object.brush", "Object: Brush Over It"),
        ("object.rect", "Object: Draw a Box"),
    ] {
        add(format!("masking.{id}"), name.into(), MASKING, None);
    }
    let lens = Some("Lens Corrections");
    for (id, name) in [
        ("tab.profile", "Lens Corrections: Show Profile"),
        ("tab.manual", "Lens Corrections: Show Manual"),
        ("ca", "Remove Chromatic Aberration"),
        ("profile", "Enable Profile Corrections"),
        ("setup.auto", "Lens Profile Setup: Auto"),
        ("setup.custom", "Lens Profile Setup: Custom"),
        ("constrain", "Lens Corrections: Constrain Crop"),
        ("download", "Download the Lens Database"),
        ("update", "Update the Lens Database"),
    ] {
        add(format!("lens.{id}"), name.into(), LENS, lens);
    }
    for (id, name) in [
        ("lock", "Crop: Lock Aspect"),
        ("straighten", "Crop: Straighten Tool"),
        ("auto", "Crop: Auto Straighten"),
        ("constrain", "Crop: Constrain to Image"),
    ] {
        add(format!("crop.{id}"), name.into(), CROP, None);
    }
    use imaging::develop::{DustPolarity, DustSource, SpotMode};
    for m in [SpotMode::Remove, SpotMode::Heal, SpotMode::Clone] {
        add(format!("remove.mode.{m:?}"), format!("Spot Mode: {}", m.label()), REMOVE, None);
    }
    for (id, name) in [
        ("best", "Remove: Best Quality"),
        ("depth", "Remove: Refine with Depth"),
        ("area.add", "Spot Area: Add"),
        ("area.subtract", "Spot Area: Subtract"),
        ("refill", "Refill the Spot"),
        ("earlier", "The Spot's Earlier Fill"),
        ("later", "The Spot's Later Fill"),
        ("sample", "Remove: Sample From"),
        ("reset", "Delete Every Spot"),
        ("dust.find", "Find Sensor Dust"),
        ("dust.clear", "Clear Found Sensor Dust"),
        ("scratches", "Dust & Scratches: On / Off"),
        ("scratches.recalculate", "Dust & Scratches: Recalculate"),
        ("scratches.ai", "Dust & Scratches: Use AI"),
        ("scratches.show", "Dust & Scratches: Show Found"),
        ("scratches.subtract", "Dust & Scratches: Subtract Brush"),
        ("scratches.clear", "Dust & Scratches: Clear Subtracted"),
        ("people.find", "Find People"),
        ("people.shadows", "Find People: Shadows"),
        ("people.none", "People: Leave Everyone In"),
        ("people.all", "People: Remove Everyone Found"),
        ("people.remove", "Remove the People Picked"),
    ] {
        add(format!("remove.{id}"), name.into(), REMOVE, None);
    }
    for s in DustSource::ALL {
        add(
            format!("remove.scratches.source.{s:?}"),
            format!("Dust & Scratches: Find in {}", s.label()),
            REMOVE,
            None,
        );
    }
    for p in DustPolarity::ALL {
        add(
            format!("remove.scratches.defects.{p:?}"),
            format!("Dust & Scratches: Defects {}", p.label()),
            REMOVE,
            None,
        );
    }
    for (id, name) in super::crop::aspect_controls() {
        add(id.trim_start_matches("develop.").to_owned(), name, CROP, None);
    }
    for (id, name) in super::masking::menu_controls() {
        add(id.trim_start_matches("develop.").to_owned(), name, MASKING, None);
    }
    out
}

/// The Basic panel's sliders out of the box: Alt+letter raises, Alt+Shift+letter lowers.
fn slider_default(s: Slider) -> Option<Key> {
    Some(match s {
        Slider::Temp => Key::T,
        Slider::Tint => Key::I,
        Slider::Exposure => Key::E,
        Slider::Contrast => Key::C,
        Slider::Highlights => Key::H,
        Slider::Shadows => Key::S,
        Slider::Whites => Key::W,
        Slider::Blacks => Key::B,
        Slider::Texture => Key::X,
        Slider::Clarity => Key::L,
        Slider::Dehaze => Key::Z,
        Slider::Vibrance => Key::V,
        Slider::Saturation => Key::A,
        _ => return None,
    })
}

/// How far a slider's key moves it out of the box.
pub fn default_step(s: Slider) -> f32 {
    match s {
        // In Kelvin (a relative Temp moves by a fiftieth of it, see [`step`]).
        Slider::Temp => 50.0,
        Slider::Exposure | Slider::CropAngle => 0.05,
        s if s.decimals() > 0 => 10f32.powi(-(s.decimals() as i32)),
        _ => 1.0,
    }
}

/// The slider moves a plain setting: its keys work without its row being drawn (Temp and
/// Tint follow the white balance preset, a mask's sliders the shown mask, the crop angle
/// the crop: those are moved by their rows).
pub fn moves_settings(s: Slider) -> bool {
    !matches!(
        s,
        Slider::Temp
            | Slider::Tint
            | Slider::CropAngle
            | Slider::Local(..)
            | Slider::MaskAmount(_)
            | Slider::MaskFeather(..)
            | Slider::MaskDepthNear(..)
            | Slider::MaskDepthFar(..)
    )
}

const fn key(key: Key, shift: bool, alt: bool) -> Binding {
    Binding { key, ctrl: false, shift, alt }
}

/// Every command, in the order they are listed.
pub fn commands() -> &'static [Command] {
    static ALL: OnceLock<Vec<Command>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut out: Vec<Command> = Vec::new();
        for s in SHORTCUTS {
            // One action with several keys (Redo: Ctrl+Y and Ctrl+Shift+Z) is one command.
            if let Some(c) = out.iter_mut().find(|c| c.run == Run::Action(s.action) && c.scope == s.scope) {
                c.defaults.push(s.binding);
                continue;
            }
            out.push(Command {
                id: action_id(s.action),
                name: s.description.to_owned(),
                section: s.group,
                panel: None,
                scope: s.scope,
                run: Run::Action(s.action),
                defaults: vec![s.binding],
            });
        }
        // Any slider shown, by keys that work for all (as in LrC).
        for (id, name, run, keys) in [
            (
                "slider.pick.previous",
                "Pick the previous slider",
                Run::PickSlider(-1),
                vec![key(Key::Comma, false, false)],
            ),
            (
                "slider.pick.next",
                "Pick the next slider",
                Run::PickSlider(1),
                vec![key(Key::Period, false, false)],
            ),
            (
                "slider.picked.up",
                "Raise the picked slider",
                Run::StepPicked(1),
                vec![key(Key::Plus, false, false), key(Key::Equals, false, false)],
            ),
            (
                "slider.picked.down",
                "Lower the picked slider",
                Run::StepPicked(-1),
                vec![key(Key::Minus, false, false)],
            ),
        ] {
            out.push(Command {
                id: id.into(),
                name: name.into(),
                section: SLIDERS,
                panel: None,
                scope: Scope::Develop,
                run,
                defaults: keys,
            });
        }
        for (id, name, section, scope) in super::app_keys::items() {
            out.push(Command { id, name, section, panel: None, scope, run: Run::App, defaults: Vec::new() });
        }
        for (id, name, section, panel) in controls() {
            out.push(Command {
                id,
                name,
                section,
                panel,
                scope: Scope::Develop,
                run: Run::Control,
                defaults: Vec::new(),
            });
        }
        for (slider, name, section, panel) in sliders() {
            let letter = slider_default(slider);
            for up in [true, false] {
                out.push(Command {
                    id: format!("{}.{}", slider_id(slider), if up { "up" } else { "down" }),
                    name: format!("{name}: {}", if up { "Raise" } else { "Lower" }),
                    section,
                    panel,
                    scope: Scope::Develop,
                    run: Run::Slider { slider, up },
                    defaults: letter.map(|k| vec![key(k, !up, true)]).unwrap_or_default(),
                });
            }
        }
        out
    })
}

/// A key combination as saved: the key by its name, so that the file reads well and
/// survives changes to the key list's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBinding {
    pub key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
}

impl From<Binding> for SavedBinding {
    fn from(b: Binding) -> Self {
        SavedBinding { key: b.key.name().to_owned(), ctrl: b.ctrl, shift: b.shift, alt: b.alt }
    }
}

impl SavedBinding {
    /// `None` for a key this version does not know.
    pub fn binding(&self) -> Option<Binding> {
        Some(Binding { key: Key::from_name(&self.key)?, ctrl: self.ctrl, shift: self.shift, alt: self.alt })
    }
}

/// The user's changes to the keys: per command id the keys it has instead of its
/// defaults (empty: none), and per slider ([`slider_id`]) its step.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    pub bindings: BTreeMap<String, Vec<SavedBinding>>,
    pub steps: BTreeMap<String, f32>,
}

impl Keymap {
    /// The keys `c` has.
    pub fn bindings(&self, c: &Command) -> Vec<Binding> {
        match self.bindings.get(&c.id) {
            Some(saved) => saved.iter().filter_map(SavedBinding::binding).collect(),
            None => c.defaults.clone(),
        }
    }

    /// Give `c` these keys (its defaults: forget the change).
    pub fn set(&mut self, c: &Command, keys: &[Binding]) {
        if keys == c.defaults.as_slice() {
            self.bindings.remove(&c.id);
        } else {
            self.bindings.insert(c.id.clone(), keys.iter().map(|b| SavedBinding::from(*b)).collect());
        }
    }

    /// How far a key moves `slider`.
    pub fn step(&self, slider: Slider) -> f32 {
        self.steps.get(&slider_id(slider)).copied().unwrap_or_else(|| default_step(slider_key(slider)))
    }

    /// Set how far a key moves `slider` (its default: forget the change).
    pub fn set_step(&mut self, slider: Slider, step: f32) {
        let id = slider_id(slider);
        if (step - default_step(slider_key(slider))).abs() < 1e-6 || step <= 0.0 {
            self.steps.remove(&id);
        } else {
            self.steps.insert(id, step);
        }
    }

    /// The commands that `key` would also run where `scope` is (other than `except`).
    pub fn clashes(&self, key: Binding, scope: Scope, except: &str) -> Vec<&'static Command> {
        commands()
            .iter()
            .filter(|c| c.id != except && c.scope.overlaps(scope) && self.bindings(c).contains(&key))
            .collect()
    }
}

static CURRENT: LazyLock<RwLock<Keymap>> = LazyLock::new(|| RwLock::new(Keymap::default()));

/// The keymap in use.
pub fn current() -> Keymap {
    CURRENT.read().map(|k| k.clone()).unwrap_or_default()
}

/// Use `keymap` from now on.
pub fn install(keymap: &Keymap) {
    if let Ok(mut k) = CURRENT.write() {
        *k = keymap.clone();
    }
}

/// The first key `action` has, as shown in menus and tooltips.
pub fn label_for(action: Action) -> Option<String> {
    let keymap = CURRENT.read().ok()?;
    commands()
        .iter()
        .filter(|c| c.run == Run::Action(action))
        .find_map(|c| keymap.bindings(c).first().map(Binding::label))
}

/// The first key command `id` has, as shown in menus.
pub fn label_for_id(id: &str) -> Option<String> {
    let keymap = CURRENT.read().ok()?;
    commands().iter().find(|c| c.id == id).and_then(|c| keymap.bindings(c).first().map(Binding::label))
}

/// How far one key press moves `slider` (`kelvin`: Temp is in Kelvin; a relative Temp
/// moves by a fiftieth of the Kelvin step).
pub fn step(slider: Slider, kelvin: bool) -> f32 {
    let step = CURRENT.read().map_or_else(|_| default_step(slider_key(slider)), |k| k.step(slider));
    if slider == Slider::Temp && !kelvin { (step / 50.0).max(1.0) } else { step }
}

/// Slider keys waiting for their row, the picked slider, the order rows were drawn, and
/// control keys waiting for their control.
#[derive(Clone, Default)]
struct Sliders {
    /// (control id, the pass it was asked in).
    pressed: Vec<(String, u64)>,
    /// (slider, how many steps (signed), the pass it was asked in).
    waiting: Vec<(Slider, f32, u64)>,
    picked: Option<Slider>,
    /// The rows drawn this frame, and in the frame before.
    drawn: Vec<Slider>,
    before: Vec<Slider>,
}

/// A key for a slider not drawn within this many passes is dropped.
const WAIT_PASSES: u64 = 8;

fn with_sliders<R>(ctx: &egui::Context, f: impl FnOnce(&mut Sliders, u64) -> R) -> R {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let s = d.get_temp_mut_or_default::<Sliders>(egui::Id::unique("hotkeys_sliders"));
        s.waiting.retain(|w| w.2 + WAIT_PASSES >= pass);
        s.pressed.retain(|w| w.1 + WAIT_PASSES >= pass);
        f(s, pass)
    })
}

/// Press control `id` when it is next drawn.
pub fn press(ctx: &egui::Context, id: &str) {
    with_sliders(ctx, |s, pass| s.pressed.push((id.to_owned(), pass)));
}

/// Control `id`'s key was pressed (true once).
pub fn fired(ctx: &egui::Context, id: &str) -> bool {
    with_sliders(ctx, |s, _| {
        let at = s.pressed.iter().position(|p| p.0 == id);
        at.map(|i| s.pressed.remove(i)).is_some()
    })
}

/// A control with a hot key: `.hot(id)` instead of `.clicked()`, and `.hot_flip` after
/// a checkbox. Ids are listed in [`controls`]; a choice in a menu, not drawn while the
/// menu is closed, checks [`fired`] beside it instead.
pub trait Hot: Sized {
    /// Clicked, or its key pressed.
    fn hot(self, id: &str) -> bool;
    /// A checkbox: its key flips `value`.
    fn hot_flip(self, id: &str, value: &mut bool) -> Self;
}

impl Hot for egui::Response {
    fn hot(self, id: &str) -> bool {
        self.clicked() || (self.enabled() && fired(&self.ctx, id))
    }

    fn hot_flip(mut self, id: &str, value: &mut bool) -> Self {
        if self.enabled() && fired(&self.ctx, id) {
            *value = !*value;
            self.mark_changed();
        }
        self
    }
}

/// Move `slider` by `steps` when its row is next drawn.
pub fn step_when_drawn(ctx: &egui::Context, slider: Slider, steps: f32) {
    with_sliders(ctx, |s, pass| s.waiting.push((slider_key(slider), steps, pass)));
}

/// A frame starts: the rows drawn in the last one are the ones shown.
pub fn begin_frame(ctx: &egui::Context) {
    with_sliders(ctx, |s, _| s.before = std::mem::take(&mut s.drawn));
}

/// Pick the previous (`dir` −1) or next slider among those shown.
pub fn pick_next(ctx: &egui::Context, dir: i8) {
    with_sliders(ctx, |s, _| {
        let shown = &s.before;
        if shown.is_empty() {
            return;
        }
        let at = s.picked.and_then(|p| shown.iter().position(|d| *d == p));
        let next = match (at, dir > 0) {
            (Some(i), true) => (i + 1).min(shown.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
            (None, true) => 0,
            (None, false) => shown.len() - 1,
        };
        s.picked = Some(shown[next]);
    });
}

/// Move the picked slider by `steps` (when it is drawn).
pub fn step_picked(ctx: &egui::Context, steps: f32) {
    with_sliders(ctx, |s, pass| {
        if let Some(p) = s.picked {
            s.waiting.push((p, steps, pass));
        }
    });
}

/// Pick `slider` (it was clicked or dragged).
pub fn pick(ctx: &egui::Context, slider: Slider) {
    with_sliders(ctx, |s, _| s.picked = Some(slider_key(slider)));
}

/// A slider row is being drawn: the steps keys asked of it (0: none), and whether it is
/// the picked one.
pub fn drawn(ctx: &egui::Context, slider: Slider) -> (f32, bool) {
    let key = slider_key(slider);
    with_sliders(ctx, |s, _| {
        if !s.drawn.contains(&key) {
            s.drawn.push(key);
        }
        let mut steps = 0.0;
        s.waiting.retain(|w| {
            if w.0 == key {
                steps += w.1;
                false
            } else {
                true
            }
        });
        (steps, s.picked == Some(key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::shortcuts::Status;

    #[test]
    fn every_command_has_its_own_id_and_every_shortcut_a_command() {
        let all = commands();
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| b.id != a.id), "id {} twice", a.id);
        }
        for s in SHORTCUTS.iter().filter(|s| s.status == Status::Available) {
            assert!(
                all.iter().any(|c| c.run == Run::Action(s.action) && c.defaults.contains(&s.binding)),
                "{} has no command",
                s.description
            );
        }
        assert!(all.iter().any(|c| c.id == "SetRating.stars=3.advance=false"));
        let redo = all.iter().find(|c| c.run == Run::Action(Action::Redo)).unwrap();
        assert_eq!(redo.defaults.len(), 2);
        assert!(all.iter().any(|c| c.id == "slider.Exposure.up"));
        assert!(all.iter().any(|c| c.id == "slider.Hsl(Saturation,2).down"));
        assert!(all.iter().any(|c| c.id == "slider.Local(0,Exposure).up"));
    }

    #[test]
    fn no_two_commands_have_the_same_key_out_of_the_box() {
        let keymap = Keymap::default();
        for c in commands() {
            for b in &c.defaults {
                let others: Vec<&str> =
                    keymap.clashes(*b, c.scope, &c.id).iter().map(|o| o.name.as_str()).collect();
                assert!(others.is_empty(), "{} for {} and {others:?}", b.label(), c.name);
            }
        }
    }

    #[test]
    fn every_slider_has_its_keys() {
        // Every kind of slider, its mask index or not.
        let mut all: Vec<Slider> =
            Slider::WB.into_iter().chain(Slider::TONE).chain(Slider::PRESENCE).collect();
        all.extend(Slider::CURVE);
        all.extend(DetailParam::ALL.map(Slider::Detail));
        all.extend(EffectParam::ALL.map(Slider::Effect));
        all.extend(TransformParam::ALL.map(Slider::Transform));
        all.extend(LocalParam::ALL.map(|p| Slider::Local(3, p)));
        all.extend([Slider::Hsl(HslGroup::Luminance, 7), Slider::BwMix(0), Slider::GradeBalance]);
        all.extend([Slider::MaskAmount(2), Slider::MaskFeather(1, 2), Slider::MaskDepthFar(0, 1)]);
        all.extend([Slider::CropAngle, Slider::Grade(GradeWheel::Global, GradeParam::Luminance)]);
        for s in all {
            for up in [true, false] {
                assert!(
                    commands().iter().any(|c| c.run == Run::Slider { slider: slider_key(s), up }),
                    "{s:?} has no key to move it"
                );
            }
        }
        // Names say which slider: no two alike.
        let names: Vec<&str> =
            commands().iter().filter(|c| c.slider().is_some()).map(|c| c.name.as_str()).collect();
        for (i, n) in names.iter().enumerate() {
            assert!(!names[i + 1..].contains(n), "{n} twice");
        }
    }

    #[test]
    fn a_sliders_step_is_its_own_and_kept() {
        let mut keymap = Keymap::default();
        assert_eq!(keymap.step(Slider::Exposure), 0.05);
        assert_eq!(keymap.step(Slider::Contrast), 1.0);
        assert_eq!(keymap.step(Slider::Detail(DetailParam::SharpenRadius)), 0.1);
        keymap.set_step(Slider::Exposure, 0.33);
        assert_eq!(keymap.step(Slider::Exposure), 0.33);
        // A mask's slider has one step, whichever mask.
        keymap.set_step(Slider::Local(2, LocalParam::Exposure), 0.2);
        assert_eq!(keymap.step(Slider::Local(0, LocalParam::Exposure)), 0.2);
        let json = serde_json::to_string(&keymap).unwrap();
        assert_eq!(serde_json::from_str::<Keymap>(&json).unwrap(), keymap);
        keymap.set_step(Slider::Exposure, 0.05);
        assert!(!keymap.steps.contains_key("slider.Exposure"));
    }

    #[test]
    fn every_key_is_saved_and_read_back_by_name() {
        for key in Key::ALL {
            let b = Binding { key: *key, ctrl: true, shift: false, alt: true };
            let saved = SavedBinding::from(b);
            assert_eq!(saved.binding(), Some(b), "{}", key.name());
        }
        assert_eq!(
            SavedBinding { key: "NoSuchKey".into(), ctrl: false, shift: false, alt: false }.binding(),
            None
        );
    }

    #[test]
    fn a_users_keys_replace_the_defaults_and_are_kept() {
        let flag = commands().iter().find(|c| c.id == "SetFlag.flag=Picked.advance=false").unwrap();
        let mut keymap = Keymap::default();
        assert_eq!(keymap.bindings(flag), flag.defaults);
        let other = Binding { key: Key::F9, ctrl: false, shift: true, alt: false };
        keymap.set(flag, &[other]);
        assert_eq!(keymap.bindings(flag), vec![other]);
        // Saved and read back.
        let json = serde_json::to_string(&keymap).unwrap();
        let back: Keymap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, keymap);
        assert!(!json.contains("\"ctrl\""), "only what is on is written: {json}");
        // None at all is kept as none; the defaults again forget the change.
        keymap.set(flag, &[]);
        assert!(keymap.bindings(flag).is_empty());
        keymap.set(flag, &flag.defaults);
        assert_eq!(keymap, Keymap::default());
    }

    #[test]
    fn a_key_already_used_where_it_would_clash_is_found() {
        let keymap = Keymap::default();
        let p = Binding { key: Key::P, ctrl: false, shift: false, alt: false };
        let names: Vec<&str> =
            keymap.clashes(p, Scope::Develop, "").iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Flag as Pick"]);
        // `\` is the Filter bar in Library and Before / After in Develop: no clash between them.
        let backslash = Binding { key: Key::Backslash, ctrl: false, shift: false, alt: false };
        let before_after = commands().iter().find(|c| c.run == Run::Action(Action::BeforeAfter)).unwrap();
        assert!(keymap.clashes(backslash, Scope::Develop, &before_after.id).is_empty());
        assert_eq!(keymap.clashes(backslash, Scope::All, "").len(), 2);
    }

    #[test]
    fn keys_pick_the_sliders_shown_in_order_and_move_the_picked_one() {
        let ctx = egui::Context::default();
        // As the app does: a frame starts, its keys are handled, then rows are drawn.
        let frame = |keys: &dyn Fn(), shown: &[Slider]| -> Vec<(f32, bool)> {
            ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
            begin_frame(&ctx);
            keys();
            shown.iter().map(|s| drawn(&ctx, *s)).collect()
        };
        let none = || {};
        let rows = [Slider::Exposure, Slider::Contrast, Slider::Local(1, LocalParam::Clarity)];
        frame(&none, &rows);
        assert_eq!(frame(&|| pick_next(&ctx, 1), &rows)[0], (0.0, true), "the first shown");
        let last = || {
            for _ in 0..3 {
                pick_next(&ctx, 1);
            }
            step_picked(&ctx, -1.0);
        };
        assert_eq!(frame(&last, &rows)[2], (-1.0, true), "the last, at most, moved once");
        assert_eq!(frame(&none, &rows)[2], (0.0, true), "and only once");
        assert_eq!(frame(&|| pick_next(&ctx, -1), &rows)[1], (0.0, true), "back one");
        // A key for a slider not shown waits for its row a few passes, then is dropped.
        let mask = || step_when_drawn(&ctx, Slider::Local(4, LocalParam::Clarity), 2.0);
        assert_eq!(frame(&mask, &rows)[2].0, 2.0, "a mask's slider, whichever mask is shown");
        frame(&|| step_when_drawn(&ctx, Slider::Whites, 1.0), &rows);
        for _ in 0..WAIT_PASSES + 2 {
            frame(&none, &rows);
        }
        assert_eq!(frame(&none, &[Slider::Whites])[0].0, 0.0);
    }
}

#[cfg(test)]
mod every_control {
    //! The rule (see `.claude/rules/hotkeys.md`): every button, checkbox and choice in the
    //! panels, toolbars and menus has a hot key. A control shows it has one by going
    //! through [`super::Hot`] (`.hot(id)`, `.hot_flip(id, ..)`), by running an `Action::`,
    //! or by a `// key: <id>` comment naming the command whose key does it (a menu's choice,
    //! checked with `hotkeys::fired` beside the menu). Exempt: a control marked
    //! `// no hotkey: <why>`, a function marked `// hotkeys: none (<why>)` above it (a
    //! dialog), and the files in [`EXEMPT`].

    use super::commands;

    /// Files that are dialogs (or the shortcuts editor) through and through.
    const EXEMPT: [&str; 5] = ["export.rs", "import.rs", "edit_in.rs", "keymap.rs", "icons.rs"];
    /// What makes a control.
    const CONTROLS: [&str; 9] = [
        "Button::new(",
        ".button(",
        "small_button(",
        "checkbox(",
        "selectable_label(",
        "selectable_value(",
        "pill(ui",
        "mode_button(ui",
        "toggle_switch(ui",
    ];
    /// What gives it a key, in the statement.
    const KEYED: [&str; 4] = [".hot(", ".hot_flip(", "Action::", "menu_cmd("];

    /// `id` is a command's id, or (ending in `*`) the start of some.
    fn known(id: &str) -> bool {
        match id.strip_suffix('*') {
            Some(start) => commands().iter().any(|c| c.id.starts_with(start)),
            None => commands().iter().any(|c| c.id == id),
        }
    }

    /// The controls without a key, and the `// key:` comments naming no command.
    pub(super) fn unkeyed() -> Vec<String> {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui");
        let mut out = Vec::new();
        let mut files: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !name.ends_with(".rs") || EXEMPT.contains(&name.as_str()) || name == "hotkeys.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let mut exempt_fn = false;
            for (i, line) in lines.iter().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                // A function at the file's top level starts: is it marked?
                let top = ["fn ", "pub fn ", "pub(super) fn ", "pub(crate) fn "];
                if top.iter().any(|t| line.starts_with(t)) {
                    exempt_fn = lines[i.saturating_sub(6)..i].iter().any(|l| l.contains("hotkeys: none"));
                    continue;
                }
                if let Some(id) =
                    line.split("// key: ").nth(1).map(|r| r.split_whitespace().next().unwrap_or(""))
                {
                    if !known(id) {
                        out.push(format!("{name}:{}: no command {id}", i + 1));
                    }
                    continue;
                }
                if exempt_fn || !CONTROLS.iter().any(|c| code.contains(c)) || code.contains("fn ") {
                    continue;
                }
                let after = lines[i..(i + 7).min(lines.len())].join(
                    "
",
                );
                let marked = lines[i.saturating_sub(3)..(i + 2).min(lines.len())]
                    .iter()
                    .any(|l| l.contains("no hotkey") || l.contains("// key: "));
                if marked || KEYED.iter().any(|k| after.contains(k)) {
                    continue;
                }
                out.push(format!("{name}:{}: {}", i + 1, line.trim()));
            }
        }
        out
    }

    #[test]
    fn every_control_has_a_hot_key() {
        let missing = unkeyed();
        assert!(
            missing.is_empty(),
            "controls without a hot key (or `// no hotkey: why`):
{}",
            missing.join(
                "
"
            )
        );
    }
}
