//! Edit & Fill: Fill form fields and add text, check marks and dates.
//!
//! Positions are in display space: points, origin at the top-left corner of the page as
//! shown, y running down. The small drawing helpers at the bottom of this file are shared
//! with Sign, Watermark, Page Numbers and Redact.

use super::flatten::{self, FieldKind, FormField};
use crate::doc::{self, StdFont, fmt};
use crate::img::{self, Encoding};
use crate::text::Rect4;
use crate::{Ctx, Error, Outcome, Result};
use lopdf::{Document, Object, ObjectId, dictionary};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Options {
    /// Form field full name (as `list_fields` reports it) to its new value. For a check
    /// box, "true", "yes" or "on" ticks it and anything else clears it; for radio buttons
    /// and choice fields the value is one of the field's `options`.
    pub fields: BTreeMap<String, String>,
    /// Values for single places of text or list fields that show in several places. In a
    /// PDF the places of one field share its value; each place listed here becomes a field
    /// of its own first, named after the original with a number, so it holds its own value.
    pub places: Vec<PlaceValue>,
    /// Things to draw on pages, in order: later items are drawn over earlier ones.
    pub items: Vec<Item>,
    /// After filling, turn the form fields into fixed page content.
    pub flatten: bool,
}

/// A value for one place of a field. `widget` is the place's `index` in `FieldInfo::widgets`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PlaceValue {
    pub field: String,
    pub widget: usize,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ItemKind {
    /// `text` with its top-left corner at `x`, `y`. Wraps at `w` when `w` is above zero.
    #[default]
    Text,
    /// A tick drawn inside the `w` by `h` box.
    Check,
    /// A cross drawn inside the `w` by `h` box.
    Cross,
    /// Like `text`; shows today's date when `text` is empty.
    Date,
    /// A see-through rectangle multiplied over what is underneath. Yellow unless `color` is set.
    Highlight,
    /// The picture in `image`, filling the box. A zero `w` or `h` follows the picture's shape.
    Image,
    /// An opaque rectangle, white unless `color` is set. It covers content; it does not remove it.
    Whiteout,
    /// A filled dot inside the `w` by `h` box.
    Dot,
    /// An oval outline that touches the sides of the `w` by `h` box, for circling something.
    Circle,
    /// A straight line through the middle of the `w` by `h` box, along its longer side.
    Line,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Item {
    pub kind: ItemKind,
    /// 1-based page number.
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    /// Font size in points.
    pub size: f32,
    /// Leave out for the usual colour of the kind: black, yellow for a highlight, white for whiteout.
    pub color: Option<[u8; 3]>,
    pub image: Option<PathBuf>,
    pub font: StdFont,
    /// 0 means the usual opacity of the kind: 0.45 for a highlight, 1 for everything else.
    pub opacity: f32,
}

impl Default for Item {
    fn default() -> Self {
        Item { kind: ItemKind::Text, page: 1, x: 0.0, y: 0.0, w: 0.0, h: 0.0, text: String::new(), size: 11.0, color: None, image: None, font: StdFont::Helvetica, opacity: 0.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FieldType {
    Text,
    Checkbox,
    Radio,
    Choice,
    Signature,
    Button,
}

impl FieldType {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::Text => "text",
            FieldType::Checkbox => "checkbox",
            FieldType::Radio => "radio",
            FieldType::Choice => "choice",
            FieldType::Signature => "signature",
            FieldType::Button => "button",
        }
    }
}

/// One form field, as the UI lists it.
#[derive(Debug, Clone, Serialize)]
pub struct FieldInfo {
    /// Fully qualified name; the key to use in `Options::fields`.
    pub name: String,
    pub kind: FieldType,
    /// Current value. A check box reads "true" or "false"; a radio group reads the chosen option or "".
    pub value: String,
    /// 1-based page of the field's first widget, 0 when it is on no page.
    pub page: usize,
    /// Where the first widget sits, in display space.
    pub rect: Rect4,
    /// Every place the field shows on a page. A radio group has one per button.
    pub widgets: Vec<WidgetInfo>,
    /// Values a check box, radio group or choice field accepts.
    pub options: Vec<String>,
    pub read_only: bool,
    pub required: bool,
    pub multiline: bool,
}

/// One place a form field shows on a page.
#[derive(Debug, Clone, Serialize)]
pub struct WidgetInfo {
    /// Position among all the places of the field, the number `PlaceValue::widget` takes.
    pub index: usize,
    /// 1-based page.
    pub page: usize,
    /// In display space.
    pub rect: Rect4,
    /// For a check box or radio button: the value that switches this one on.
    pub state: String,
}

/// The form fields of a file, in the order the form defines them, so the UI can show
/// and count them. Fields nested under `/Kids` get their names joined with dots.
pub fn list_fields(path: &Path, password: Option<&str>) -> Result<Vec<FieldInfo>> {
    let doc = doc::load(path, password)?;
    Ok(field_infos(&doc))
}

fn field_infos(doc: &Document) -> Vec<FieldInfo> {
    let ids = doc::page_ids(doc);
    let mut page_of: HashMap<ObjectId, usize> = HashMap::new();
    for (i, pid) in ids.iter().enumerate() {
        for (id, _) in flatten::page_annots(doc, *pid) {
            if let Some(id) = id {
                page_of.entry(id).or_insert(i);
            }
        }
    }
    let mut out = Vec::new();
    for field in flatten::form_fields(doc) {
        let kind = match field.kind() {
            FieldKind::Text => FieldType::Text,
            FieldKind::Checkbox => FieldType::Checkbox,
            FieldKind::Radio => FieldType::Radio,
            FieldKind::Choice => FieldType::Choice,
            FieldKind::Signature => FieldType::Signature,
            FieldKind::Button => FieldType::Button,
            FieldKind::Unknown => continue,
        };
        let mut widgets = Vec::new();
        for (place, w) in field.widgets.iter().enumerate() {
            let Ok(dict) = doc.get_dictionary(*w) else { continue };
            let index = page_of.get(w).copied().or_else(|| {
                let p = dict.get(b"P").and_then(Object::as_reference).ok()?;
                ids.iter().position(|id| *id == p)
            });
            if let (Some(index), Some(r)) = (index, dict.get(b"Rect").ok().and_then(|r| doc::rect_of(doc, r))) {
                let state = flatten::on_states(doc, *w).into_iter().next().map(|s| String::from_utf8_lossy(&s).into_owned()).unwrap_or_default();
                widgets.push(WidgetInfo { index: place, page: index + 1, rect: flatten::page_rect_to_display(doc, ids[index], r), state });
            }
        }
        let (page, rect) = widgets.first().map(|w| (w.page, w.rect)).unwrap_or((0, Rect4::new(0.0, 0.0, 0.0, 0.0)));
        let states: Vec<String> = field.widgets.iter().flat_map(|w| flatten::on_states(doc, *w)).map(|s| String::from_utf8_lossy(&s).into_owned()).collect();
        let raw = field.value_text();
        let (value, options) = match kind {
            FieldType::Checkbox => ((!raw.is_empty() && raw != "Off").to_string(), dedup(states)),
            FieldType::Radio => {
                let options = if field.options.is_empty() { dedup(states) } else { field.options.iter().map(|(export, _)| export.clone()).collect() };
                (if raw == "Off" { String::new() } else { raw }, options)
            }
            FieldType::Choice => (raw, field.options.iter().map(|(export, _)| export.clone()).collect()),
            _ => (raw, Vec::new()),
        };
        out.push(FieldInfo {
            name: field.name.clone(),
            kind,
            value,
            page,
            rect,
            widgets,
            options,
            read_only: field.flags & 1 != 0,
            required: field.flags & 2 != 0,
            multiline: field.multiline(),
        });
    }
    out
}

fn dedup(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

pub fn run(inputs: &[PathBuf], out: &Path, opts: &Options, ctx: &Ctx) -> Result<Outcome> {
    let input = flatten::single_input(inputs)?;
    if opts.fields.is_empty() && opts.places.is_empty() && opts.items.is_empty() && !opts.flatten {
        return Err(Error::invalid("There is nothing to change yet. Fill in a field or add something to a page first."));
    }
    ctx.report(0.0, "Opening the file");
    let mut doc = doc::load(input, ctx.password())?;
    let ids = doc::page_ids(&doc);
    let mut notes: Vec<String> = Vec::new();
    let mut lossy = false;

    let mut values = opts.fields.clone();
    if !opts.places.is_empty() {
        for (name, value) in detach_places(&mut doc, &opts.places)? {
            values.insert(name, value);
        }
        notes.push(format!("Filled {} on {} own, apart from the other places of the same field.", flatten::plural(opts.places.len(), "place"), if opts.places.len() == 1 { "its" } else { "their" }));
    }
    if !values.is_empty() {
        ctx.report(0.1, "Filling the form");
        fill_fields(&mut doc, &values, &mut notes, &mut lossy)?;
        notes.push(format!("Filled {}.", flatten::plural(values.len(), "form field")));
    }
    ctx.check()?;
    if opts.flatten {
        let stats = flatten::flatten_doc(&mut doc, &flatten::Options { forms: true, annotations: false, keep_links: true }, ctx)?;
        notes.push(format!("Flattened {}: the form can no longer be changed.", flatten::plural(stats.fields, "form field")));
    }

    // A viewer draws form fields over the page, so anything added on top of a field would
    // end up under it. Such fields become part of the page first.
    if !opts.flatten && !opts.items.is_empty() {
        let covered = covered_widgets(&doc, &ids, &opts.items);
        if !covered.is_empty() {
            let stats = flatten::flatten_widgets(&mut doc, &covered, ctx)?;
            if stats.fields > 0 {
                notes.push(format!("{} had something added on top, so {} now part of the page and can no longer be filled in.", flatten::plural(stats.fields, "form field"), if stats.fields == 1 { "it is" } else { "they are" }));
            }
        }
    }

    // Items, page by page, drawn after the form so they sit on top of it.
    let mut fonts = Fonts::default();
    let mut images: HashMap<PathBuf, (ObjectId, u32, u32)> = HashMap::new();
    let mut per_page: BTreeMap<usize, String> = BTreeMap::new();
    let mut whiteout = false;
    for (n, item) in opts.items.iter().enumerate() {
        ctx.check()?;
        ctx.report(0.3 + 0.6 * n as f32 / opts.items.len().max(1) as f32, &format!("Adding item {} of {}", n + 1, opts.items.len()));
        if item.page == 0 || item.page > ids.len() {
            return Err(Error::invalid(format!("Item {} is on page {}, but this file has {}.", n + 1, item.page, flatten::plural(ids.len(), "page"))));
        }
        let pid = ids[item.page - 1];
        let ops = draw_item(&mut doc, pid, item, n + 1, &mut fonts, &mut images, &mut lossy)?;
        whiteout |= item.kind == ItemKind::Whiteout;
        per_page.entry(item.page).or_default().push_str(&ops);
    }
    for (page, ops) in per_page {
        if ops.is_empty() {
            continue;
        }
        let pid = ids[page - 1];
        let content = format!("{}{}", doc::cm(doc::visible_to_page(&doc, pid)), ops);
        doc::overlay(&mut doc, pid, content.into_bytes(), false)?;
    }
    if !opts.items.is_empty() {
        notes.push(format!("Added {}.", flatten::plural(opts.items.len(), "item")));
    }
    if whiteout {
        notes.push("Whiteout covers content but does not remove it from the file. Use Redact to remove it for good.".into());
    }
    if lossy {
        notes.push(LOSSY_NOTE.into());
    }

    ctx.check()?;
    ctx.report(0.95, "Saving");
    doc::save(&mut doc, out)?;
    let mut outcome = Outcome::single(out.to_path_buf(), ids.len(), crate::ctx::file_size(input));
    outcome.notes = notes;
    flatten::unlocked_note(&doc, &mut outcome);
    ctx.report(1.0, "Done");
    Ok(outcome)
}

/// The area an item covers, in display space.
fn item_rect(item: &Item) -> Rect4 {
    let boxed = |default: f32| Rect4::new(item.x, item.y, item.x + if item.w > 0.0 { item.w } else { default }, item.y + if item.h > 0.0 { item.h } else { default });
    match item.kind {
        ItemKind::Text | ItemKind::Date => {
            let size = if item.size > 0.0 { item.size } else { 11.0 };
            let text = if item.kind == ItemKind::Date && item.text.trim().is_empty() { today() } else { item.text.clone() };
            let lines = flatten::wrap(&text, item.font, size, item.w.max(0.0));
            let width = lines.iter().map(|l| item.font.width(l, size)).fold(0.0, f32::max);
            Rect4::new(item.x, item.y, item.x + width, item.y + size * 1.2 * lines.len().max(1) as f32)
        }
        ItemKind::Check | ItemKind::Cross => boxed(12.0),
        ItemKind::Dot => boxed(8.0),
        ItemKind::Image => boxed(120.0),
        ItemKind::Circle | ItemKind::Line | ItemKind::Highlight | ItemKind::Whiteout => Rect4::new(item.x, item.y, item.x + item.w.max(0.0), item.y + item.h.max(0.0)).grow(0.6),
    }
}

/// Form widgets that have an item drawn across them.
fn covered_widgets(doc: &Document, ids: &[ObjectId], items: &[Item]) -> std::collections::HashSet<ObjectId> {
    let mut out = std::collections::HashSet::new();
    for (index, pid) in ids.iter().enumerate() {
        let rects: Vec<Rect4> = items.iter().filter(|i| i.page == index + 1).map(item_rect).collect();
        if rects.is_empty() {
            continue;
        }
        for (id, annot) in flatten::page_annots(doc, *pid) {
            let (Some(id), true) = (id, annot.get(b"Subtype").and_then(Object::as_name).is_ok_and(|n| n == b"Widget")) else { continue };
            let Some(r) = annot.get(b"Rect").ok().and_then(|r| doc::rect_of(doc, r)) else { continue };
            let shown = flatten::page_rect_to_display(doc, *pid, r);
            if rects.iter().any(|item| item.intersects(&shown)) {
                out.insert(id);
            }
        }
    }
    out
}

/// Give each listed place of a field that shows in several places a field of its own, so
/// it can hold its own value. The new field sits next to the old one in the form and is
/// named after it with the place's number ("Date 2"). Returns the new names with the values.
fn detach_places(doc: &mut Document, places: &[PlaceValue]) -> Result<Vec<(String, String)>> {
    let fields = flatten::form_fields(doc);
    let mut taken: std::collections::HashSet<String> = fields.iter().map(|f| f.name.clone()).collect();
    let mut out = Vec::new();
    for place in places {
        let field = fields.iter().find(|f| f.name == place.field).ok_or_else(|| Error::invalid(format!("There is no form field called \"{}\" in this file.", place.field)))?;
        let widget = *field.widgets.get(place.widget).ok_or_else(|| Error::invalid(format!("\"{}\" has no place number {}.", place.field, place.widget + 1)))?;
        if !matches!(field.kind(), FieldKind::Text | FieldKind::Choice) {
            return Err(Error::invalid(format!("\"{}\" is not a text or list field, so its places cannot be filled one by one.", place.field)));
        }
        // A field drawn in one place is that place already.
        if field.widgets.len() == 1 || widget == field.id {
            out.push((field.name.clone(), place.value.clone()));
            continue;
        }
        let (prefix, partial) = match field.name.rsplit_once('.') {
            Some((p, last)) => (format!("{p}."), last.to_string()),
            None => (String::new(), field.name.clone()),
        };
        let mut n = place.widget + 1;
        let name = loop {
            let candidate = format!("{partial} {n}");
            if !taken.contains(&format!("{prefix}{candidate}")) {
                break candidate;
            }
            n += 1;
        };
        taken.insert(format!("{prefix}{name}"));
        let grand = doc.get_dictionary(field.id)?.get(b"Parent").and_then(Object::as_reference).ok();
        // Out of the shared field...
        if let Ok(kids) = doc.get_dictionary_mut(field.id)?.get_mut(b"Kids").and_then(Object::as_array_mut) {
            kids.retain(|k| !matches!(k, Object::Reference(id) if *id == widget));
        }
        // ...into a field of its own that keeps everything the shared field gave it.
        {
            let w = doc.get_dictionary_mut(widget)?;
            w.set("T", flatten::encode_text(&name));
            w.set("FT", Object::Name(field.ft.clone()));
            w.set("Ff", field.flags);
            if let Some(da) = &field.da {
                w.set("DA", Object::string_literal(da.clone()));
            }
            if field.quadding != 0 {
                w.set("Q", field.quadding);
            }
            if field.max_len > 0 {
                w.set("MaxLen", field.max_len);
            }
            if !field.options.is_empty() {
                let opt: Vec<Object> = field.options.iter().map(|(export, shown)| Object::Array(vec![flatten::encode_text(export), flatten::encode_text(shown)])).collect();
                w.set("Opt", opt);
            }
            w.remove(b"V");
            match grand {
                Some(g) => w.set("Parent", g),
                None => {
                    w.remove(b"Parent");
                }
            }
        }
        match grand {
            Some(g) => {
                if let Ok(kids) = doc.get_dictionary_mut(g)?.get_mut(b"Kids").and_then(Object::as_array_mut) {
                    kids.push(Object::Reference(widget));
                }
            }
            None => {
                if let Some(list) = flatten::acroform_mut(doc).and_then(|a| a.get_mut(b"Fields").ok()).and_then(|f| f.as_array_mut().ok()) {
                    list.push(Object::Reference(widget));
                }
            }
        }
        out.push((format!("{prefix}{name}"), place.value.clone()));
    }
    Ok(out)
}

fn truthy(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "1" | "checked" | "x")
}

fn fill_fields(doc: &mut Document, values: &BTreeMap<String, String>, notes: &mut Vec<String>, lossy: &mut bool) -> Result<()> {
    let fields = flatten::form_fields(doc);
    if fields.is_empty() {
        return Err(Error::invalid("This file has no form fields to fill."));
    }
    let mut helv = None;
    for (name, value) in values {
        let field: &FormField = fields.iter().find(|f| f.name == *name).ok_or_else(|| Error::invalid(format!("There is no form field called \"{name}\" in this file.")))?;
        match field.kind() {
            FieldKind::Text => {
                {
                    let dict = doc.get_dictionary_mut(field.id)?;
                    dict.set("V", flatten::encode_text(value));
                    dict.remove(b"RV");
                }
                *lossy |= loses_characters(value);
                for &w in &field.widgets {
                    flatten::write_text_appearance(doc, field, w, value, &mut helv)?;
                }
            }
            FieldKind::Choice => {
                let found = field.options.iter().position(|(export, shown)| export == value || shown == value);
                let editable = field.flags & (1 << 18) != 0;
                let (export, shown) = match found {
                    Some(i) => field.options[i].clone(),
                    None if editable || field.options.is_empty() || value.is_empty() => (value.clone(), value.clone()),
                    None => {
                        let list = field.options.iter().map(|(export, _)| export.as_str()).collect::<Vec<_>>().join(", ");
                        return Err(Error::invalid(format!("\"{value}\" is not one of the choices of \"{name}\". Choose one of: {list}.")));
                    }
                };
                {
                    let dict = doc.get_dictionary_mut(field.id)?;
                    dict.set("V", flatten::encode_text(&export));
                    match found {
                        Some(i) => dict.set("I", vec![Object::Integer(i as i64)]),
                        None => {
                            dict.remove(b"I");
                        }
                    }
                }
                *lossy |= loses_characters(&shown);
                for &w in &field.widgets {
                    flatten::write_text_appearance(doc, field, w, &shown, &mut helv)?;
                }
            }
            FieldKind::Checkbox => {
                let mut plan: Vec<(ObjectId, Vec<u8>)> = Vec::new();
                for &w in &field.widgets {
                    let state = match flatten::on_states(doc, w).into_iter().next() {
                        Some(state) => state,
                        None => flatten::write_check_appearance(doc, w)?,
                    };
                    plan.push((w, state));
                }
                // Boxes that share a name but have different on states work like radio
                // buttons: the value may name the one to tick.
                let exact = plan.iter().find(|(_, s)| s.as_slice() == value.as_bytes()).map(|(_, s)| s.clone());
                let target = exact.or_else(|| if truthy(value) { plan.first().map(|(_, s)| s.clone()) } else { None });
                set_states(doc, field.id, &plan, target)?;
            }
            FieldKind::Radio => {
                let plan: Vec<(ObjectId, Vec<u8>)> = field.widgets.iter().filter_map(|w| flatten::on_states(doc, *w).into_iter().next().map(|s| (*w, s))).collect();
                let target = if value.is_empty() || value.eq_ignore_ascii_case("off") {
                    None
                } else if let Some((_, s)) = plan.iter().find(|(_, s)| s.as_slice() == value.as_bytes()) {
                    Some(s.clone())
                } else if let Some(i) = field.options.iter().position(|(export, shown)| export == value || shown == value).filter(|i| *i < field.widgets.len()) {
                    plan.iter().find(|(w, _)| *w == field.widgets[i]).map(|(_, s)| s.clone())
                } else {
                    let list = plan.iter().map(|(_, s)| String::from_utf8_lossy(s).into_owned()).collect::<Vec<_>>().join(", ");
                    return Err(Error::invalid(format!("\"{value}\" is not one of the options of \"{name}\". Choose one of: {list}.")));
                };
                set_states(doc, field.id, &plan, target)?;
            }
            FieldKind::Signature => {
                return Err(Error::Unsupported(format!("\"{name}\" is a signature field and cannot be filled with text. Use Sign to place a signature on the page.")));
            }
            FieldKind::Button | FieldKind::Unknown => {
                return Err(Error::invalid(format!("\"{name}\" is a button and cannot hold a value.")));
            }
        }
    }
    if fields.iter().any(|f| f.kind() == FieldKind::Signature && f.value.is_some()) {
        notes.push("This file carries a digital signature. Changing the form invalidates it.".into());
    }
    // Viewers that rebuild appearances themselves (NeedAppearances) look the fonts up in
    // the form's default resources: Helv for text and ZaDb for check marks.
    let mut dr = flatten::acroform(doc).and_then(|a| a.get(b"DR").ok()).and_then(|o| doc::deref(doc, o).as_dict().ok()).cloned().unwrap_or_default();
    let mut dr_fonts = dr.get(b"Font").ok().and_then(|o| doc::deref(doc, o).as_dict().ok()).cloned().unwrap_or_default();
    for (name, base) in [("Helv", "Helvetica"), ("ZaDb", "ZapfDingbats")] {
        if !dr_fonts.has(name.as_bytes()) {
            let id = match (name, helv) {
                ("Helv", Some(id)) => id,
                ("Helv", None) => doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base, "Encoding" => "WinAnsiEncoding" }),
                _ => doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => base }),
            };
            dr_fonts.set(name, Object::Reference(id));
        }
    }
    dr.set("Font", Object::Dictionary(dr_fonts));
    if let Some(acro) = flatten::acroform_mut(doc) {
        acro.set("NeedAppearances", true);
        acro.set("DR", Object::Dictionary(dr));
        if acro.remove(b"XFA").is_some() {
            notes.push("This was an XFA form. The XFA layer was removed so that viewers show the values filled in here.".into());
        }
    }
    Ok(())
}

/// Switch each widget to its on state when that is the target, otherwise to `Off`, and
/// record the choice as the field's value.
fn set_states(doc: &mut Document, field: ObjectId, plan: &[(ObjectId, Vec<u8>)], target: Option<Vec<u8>>) -> Result<()> {
    let mut used = false;
    for (widget, state) in plan {
        // Every widget whose on state is the target is switched on; the rest go off.
        let on = target.as_ref() == Some(state);
        used |= on;
        doc.get_dictionary_mut(*widget)?.set("AS", Object::Name(if on { state.clone() } else { b"Off".to_vec() }));
    }
    let value = if used { target.unwrap_or_else(|| b"Off".to_vec()) } else { b"Off".to_vec() };
    doc.get_dictionary_mut(field)?.set("V", Object::Name(value));
    Ok(())
}

fn draw_item(
    doc: &mut Document, pid: ObjectId, item: &Item, number: usize, fonts: &mut Fonts, images: &mut HashMap<PathBuf, (ObjectId, u32, u32)>, lossy: &mut bool,
) -> Result<String> {
    let (_, page_h) = doc::visible_size(doc, pid);
    let finite = [item.x, item.y, item.w, item.h, item.size, item.opacity].iter().all(|v| v.is_finite());
    if !finite {
        return Err(Error::invalid(format!("Item {number} has a position or size that is not a number.")));
    }
    let boxed = |default: f32| -> (f32, f32, f32, f32) {
        let w = if item.w > 0.0 { item.w } else { default };
        let h = if item.h > 0.0 { item.h } else { default };
        (item.x, page_h - item.y - h, w, h)
    };
    let mut ops = String::from("q\n");
    if item.opacity > 0.0 && item.opacity < 1.0 && item.kind != ItemKind::Highlight {
        let gs = doc::ensure_alpha(doc, pid, item.opacity)?;
        ops += &format!("/{gs} gs\n");
    }
    match item.kind {
        ItemKind::Text | ItemKind::Date => {
            let text = if item.kind == ItemKind::Date && item.text.trim().is_empty() { today() } else { item.text.clone() };
            if text.trim().is_empty() {
                return Ok(String::new());
            }
            let size = if item.size > 0.0 { item.size } else { 11.0 };
            *lossy |= loses_characters(&text);
            let font = fonts.name(doc, pid, item.font)?;
            ops += &fill_rgb(item.color.unwrap_or([0, 0, 0]));
            ops += &format!("BT /{font} {} Tf\n", fmt(size));
            let mut y = page_h - item.y - ascent(item.font) * size;
            for line in flatten::wrap(&text, item.font, size, item.w.max(0.0)) {
                ops += &format!("1 0 0 1 {} {} Tm {} Tj\n", fmt(item.x), fmt(y), doc::pdf_string(&line));
                y -= size * 1.2;
            }
            ops += "ET\n";
        }
        ItemKind::Check | ItemKind::Cross => {
            let (x, y, w, h) = boxed(12.0);
            let width = (w.min(h) * 0.1).max(0.8);
            ops += &stroke_rgb(item.color.unwrap_or([0, 0, 0]));
            ops += &format!("{} w 1 J 1 j\n", fmt(width));
            let p = |fx: f32, fy: f32| format!("{} {}", fmt(x + fx * w), fmt(y + fy * h));
            if item.kind == ItemKind::Check {
                ops += &format!("{} m {} l {} l S\n", p(0.15, 0.5), p(0.4, 0.2), p(0.85, 0.85));
            } else {
                ops += &format!("{} m {} l S {} m {} l S\n", p(0.15, 0.15), p(0.85, 0.85), p(0.15, 0.85), p(0.85, 0.15));
            }
        }
        ItemKind::Highlight | ItemKind::Whiteout => {
            if item.w <= 0.0 || item.h <= 0.0 {
                return Err(Error::invalid(format!("Item {number} needs a width and a height.")));
            }
            let (x, y, w, h) = boxed(0.0);
            if item.kind == ItemKind::Highlight {
                let alpha = if item.opacity > 0.0 { item.opacity.min(1.0) } else { 0.45 };
                let gs = dictionary! { "Type" => "ExtGState", "BM" => "Multiply", "ca" => Object::Real(alpha), "CA" => Object::Real(alpha) };
                let name = doc::add_resource(doc, pid, "ExtGState", "ShG", Object::Dictionary(gs))?;
                ops += &format!("/{name} gs\n");
                ops += &fill_rgb(item.color.unwrap_or([255, 226, 0]));
            } else {
                ops += &fill_rgb(item.color.unwrap_or([255, 255, 255]));
            }
            ops += &format!("{} {} {} {} re f\n", fmt(x), fmt(y), fmt(w), fmt(h));
        }
        ItemKind::Dot => {
            let (x, y, w, h) = boxed(8.0);
            ops += &fill_rgb(item.color.unwrap_or([0, 0, 0]));
            ops += &oval(x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
            ops += "f
";
        }
        ItemKind::Circle => {
            if item.w <= 0.0 || item.h <= 0.0 {
                return Err(Error::invalid(format!("Item {number} needs a width and a height.")));
            }
            let (x, y, w, h) = boxed(0.0);
            let width = STROKE.min(w.min(h) / 2.0);
            ops += &stroke_rgb(item.color.unwrap_or([0, 0, 0]));
            ops += &format!("{} w
", fmt(width));
            ops += &oval(x + w / 2.0, y + h / 2.0, (w - width) / 2.0, (h - width) / 2.0);
            ops += "S
";
        }
        ItemKind::Line => {
            if item.w <= 0.0 && item.h <= 0.0 {
                return Err(Error::invalid(format!("Item {number} needs a width or a height.")));
            }
            let (x, y, w, h) = (item.x, page_h - item.y - item.h.max(0.0), item.w.max(0.0), item.h.max(0.0));
            ops += &stroke_rgb(item.color.unwrap_or([0, 0, 0]));
            ops += &format!("{} w 1 J
", fmt(STROKE));
            if w >= h {
                ops += &format!("{} {} m {} {} l S
", fmt(x), fmt(y + h / 2.0), fmt(x + w), fmt(y + h / 2.0));
            } else {
                ops += &format!("{} {} m {} {} l S
", fmt(x + w / 2.0), fmt(y), fmt(x + w / 2.0), fmt(y + h));
            }
        }
        ItemKind::Image => {
            let path = image_path(&item.image).ok_or_else(|| Error::invalid(format!("Item {number} is an image but no image file was chosen.")))?;
            let (id, iw, ih) = match images.get(path) {
                Some(found) => *found,
                None => {
                    let added = embed_image(doc, path)?;
                    images.insert(path.to_path_buf(), added);
                    added
                }
            };
            let aspect = ih as f32 / iw as f32;
            let (w, h) = match (item.w > 0.0, item.h > 0.0) {
                (true, true) => (item.w, item.h),
                (true, false) => (item.w, item.w * aspect),
                (false, true) => (item.h / aspect, item.h),
                (false, false) => (120.0, 120.0 * aspect),
            };
            let name = doc::ensure_xobject(doc, pid, id)?;
            ops += &img::draw(&name, item.x, page_h - item.y - h, w, h);
        }
    }
    ops += "Q\n";
    Ok(ops)
}

/// Line width of a drawn circle or line, in points.
pub const STROKE: f32 = 1.2;

/// Path of an oval centred on `cx`, `cy`, as four curves.
fn oval(cx: f32, cy: f32, rx: f32, ry: f32) -> String {
    let (kx, ky) = (rx * 0.5523, ry * 0.5523);
    let p = |x: f32, y: f32| format!("{} {}", fmt(x), fmt(y));
    format!(
        "{} m {} {} {} c {} {} {} c {} {} {} c {} {} {} c h
",
        p(cx + rx, cy),
        p(cx + rx, cy + ky), p(cx + kx, cy + ry), p(cx, cy + ry),
        p(cx - kx, cy + ry), p(cx - rx, cy + ky), p(cx - rx, cy),
        p(cx - rx, cy - ky), p(cx - kx, cy - ry), p(cx, cy - ry),
        p(cx + kx, cy - ry), p(cx + rx, cy - ky), p(cx + rx, cy),
    )
}

// ---------------------------------------------------------------------------
// Shared drawing helpers
// ---------------------------------------------------------------------------

pub(crate) const LOSSY_NOTE: &str = "Some characters are not available in the built-in fonts and were replaced with a question mark.";

/// Fill colour operator for an RGB colour.
pub(crate) fn fill_rgb(c: [u8; 3]) -> String {
    format!("{} {} {} rg\n", fmt(c[0] as f32 / 255.0), fmt(c[1] as f32 / 255.0), fmt(c[2] as f32 / 255.0))
}

/// Stroke colour operator for an RGB colour.
pub(crate) fn stroke_rgb(c: [u8; 3]) -> String {
    format!("{} {} {} RG\n", fmt(c[0] as f32 / 255.0), fmt(c[1] as f32 / 255.0), fmt(c[2] as f32 / 255.0))
}

/// Height of the capital letters of a standard font, as a fraction of the font size.
pub(crate) fn ascent(font: StdFont) -> f32 {
    match font {
        StdFont::Helvetica | StdFont::HelveticaBold => 0.718,
        StdFont::Times => 0.662,
        StdFont::Courier => 0.562,
    }
}

/// True when the standard fonts cannot show every character of `text`.
pub(crate) fn loses_characters(text: &str) -> bool {
    text.chars().filter(|c| !c.is_control()).any(|c| c != '?' && doc::winansi(&c.to_string()) == b"?")
}

/// A path option the UI may send as an empty string when nothing is chosen.
pub(crate) fn image_path(path: &Option<PathBuf>) -> Option<&Path> {
    path.as_deref().filter(|p| !p.as_os_str().is_empty())
}

/// Standard fonts for a run: one font object per font for the whole document, and one
/// resource name per page.
#[derive(Default)]
pub(crate) struct Fonts {
    objects: HashMap<StdFont, ObjectId>,
    names: HashMap<(ObjectId, StdFont), String>,
}

impl Fonts {
    /// The resource name of `font` on this page, adding it when needed.
    pub fn name(&mut self, doc: &mut Document, page_id: ObjectId, font: StdFont) -> Result<String> {
        if let Some(name) = self.names.get(&(page_id, font)) {
            return Ok(name.clone());
        }
        let name = match self.objects.get(&font) {
            Some(id) => doc::add_resource(doc, page_id, "Font", "ShF", Object::Reference(*id))?,
            None => {
                let name = doc::ensure_font(doc, page_id, font)?;
                let id = doc::resources_mut(doc, page_id)?.get(b"Font").and_then(Object::as_dict).and_then(|f| f.get(name.as_bytes())).and_then(Object::as_reference);
                if let Ok(id) = id {
                    self.objects.insert(font, id);
                }
                name
            }
        };
        self.names.insert((page_id, font), name.clone());
        Ok(name)
    }
}

/// Add an image file to the document. A JPEG goes in as it is; anything else is stored
/// losslessly with its transparency. Camera orientation is applied. Returns the object
/// id and the pixel size.
pub(crate) fn embed_image(doc: &mut Document, path: &Path) -> Result<(ObjectId, u32, u32)> {
    use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, metadata::Orientation};
    let bytes = doc::read_file(path)?;
    let unreadable = || Error::invalid(format!("{} is not an image this tool can read. Use a PNG or JPEG file.", path.display()));
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().map_err(|_| unreadable())?;
    let format = reader.format();
    let mut decoder = reader.into_decoder().map_err(|_| unreadable())?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| unreadable())?;
    image.apply_orientation(orientation);
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::invalid(format!("{} is an empty image.", path.display())));
    }
    let id = if format == Some(ImageFormat::Jpeg) {
        match jpeg_components(&bytes) {
            Some(components @ (1 | 3)) if orientation == Orientation::NoTransforms => img::add_jpeg_bytes(doc, bytes, w, h, components),
            _ => img::add_image(doc, &image, Encoding::Jpeg(92))?,
        }
    } else {
        img::add_image(doc, &image, Encoding::Flate)?
    };
    Ok((id, w, h))
}

/// Pixel size of an image file as it will be placed (camera orientation applied),
/// read from its header without decoding the picture.
pub(crate) fn image_size(path: &Path) -> Result<(u32, u32)> {
    use image::{ImageDecoder, metadata::Orientation};
    let unreadable = || Error::invalid(format!("{} is not an image this tool can read. Use a PNG or JPEG file.", path.display()));
    let reader = image::ImageReader::open(path).map_err(|e| Error::read(path, e))?.with_guessed_format().map_err(|e| Error::read(path, e))?;
    let mut decoder = reader.into_decoder().map_err(|_| unreadable())?;
    let (w, h) = decoder.dimensions();
    let turned = matches!(decoder.orientation().unwrap_or(Orientation::NoTransforms), Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH);
    Ok(if turned { (h, w) } else { (w, h) })
}

/// Number of colour components in a JPEG, read from its frame header.
fn jpeg_components(b: &[u8]) -> Option<u8> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        match marker {
            0xFF => {
                i += 1;
                continue;
            }
            0x01 | 0xD0..=0xD7 => {
                i += 2;
                continue;
            }
            0xDA | 0xD9 => return None,
            _ => {}
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            return b.get(i + 9).copied();
        }
        i += 2 + u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
    }
    None
}

/// Today's date as YYYY-MM-DD, in local time where the system can say what that is.
pub fn today() -> String {
    let (y, m, d) = local_date().unwrap_or_else(utc_date);
    format!("{y:04}-{m:02}-{d:02}")
}

fn utc_date() -> (i32, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    civil_from_days(secs.div_euclid(86_400))
}

/// Days since 1970-01-01 to a calendar date (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y as i32, m as u32, d as u32)
}

#[cfg(windows)]
fn local_date() -> Option<(i32, u32, u32)> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut t = SystemTime::default();
    // SAFETY: GetLocalTime writes one SYSTEMTIME (eight 16-bit fields) to the pointer and cannot fail.
    unsafe { GetLocalTime(&mut t) };
    (t.year >= 1970 && (1..=12).contains(&t.month) && (1..=31).contains(&t.day)).then_some((t.year as i32, t.month as u32, t.day as u32))
}

#[cfg(unix)]
fn local_date() -> Option<(i32, u32, u32)> {
    use std::ffi::c_int;
    // The leading fields of `struct tm` are the same on every Unix; the tail differs
    // (time zone offset and name), so leave generous room for it.
    #[repr(C)]
    struct Tm {
        sec: c_int,
        min: c_int,
        hour: c_int,
        mday: c_int,
        mon: c_int,
        year: c_int,
        rest: [u64; 12],
    }
    unsafe extern "C" {
        fn time(t: *mut i64) -> i64;
        fn localtime_r(t: *const i64, tm: *mut Tm) -> *mut Tm;
    }
    // `time_t` is 32 or 64 bits depending on the platform. The C library writes it into
    // this buffer and reads it back itself, so its width never matters here.
    let mut now = [0i64; 2];
    let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, rest: [0; 12] };
    // SAFETY: both pointers are valid, aligned and large enough for `time_t` and `struct tm`.
    let ok = unsafe {
        time(now.as_mut_ptr());
        !localtime_r(now.as_ptr(), &mut tm).is_null()
    };
    (ok && (0..=11).contains(&tm.mon) && (1..=31).contains(&tm.mday)).then_some((tm.year + 1900, tm.mon as u32 + 1, tm.mday as u32))
}

#[cfg(not(any(windows, unix)))]
fn local_date() -> Option<(i32, u32, u32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_728), (2026, 10, 2));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn today_looks_like_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t[..4].parse::<i32>().unwrap() >= 2024);
        assert_eq!(&t[4..5], "-");
    }

    #[test]
    fn spots_characters_the_fonts_lack() {
        assert!(!loses_characters("Zoë? — ok"));
        assert!(loses_characters("日本"));
    }
}
