//! Test documents, generated rather than checked in. Used by the tests and by
//! `shorui-cli fixtures <dir>`. Every text page carries a `MARK-...` line so a test can
//! tell which source page ended up where.

use crate::doc::{self, fmt, pdf_string};
use crate::img::{self, Encoding};
use crate::{Error, Result};
use image::{DynamicImage, Rgb, RgbImage, Rgba, RgbaImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};
use std::path::{Path, PathBuf};

pub const A4: (f32, f32) = (595.0, 842.0);

/// A small helper for laying out fixture pages.
pub struct Builder {
    pub doc: Document,
    pub root: ObjectId,
    pub regular: ObjectId,
    pub bold: ObjectId,
    pub pages: Vec<ObjectId>,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}

impl Builder {
    pub fn new() -> Self {
        let (mut doc, root) = doc::new_document();
        let regular = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
        let bold = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica-Bold", "Encoding" => "WinAnsiEncoding" });
        Builder { doc, root, regular, bold, pages: Vec::new() }
    }

    /// Add a page with raw content. Fonts `/F1` (regular) and `/F2` (bold) are available.
    pub fn page(&mut self, size: (f32, f32), content: String) -> ObjectId {
        let content_id = self.doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
        let page = self.doc.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(size.0), Object::Real(size.1)],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => self.regular, "F2" => self.bold } },
            "Contents" => content_id,
        });
        self.pages.push(page);
        page
    }

    pub fn finish(mut self) -> Document {
        let pages = self.pages.clone();
        doc::append_pages(&mut self.doc, self.root, &pages).expect("fixture page tree");
        self.doc
    }
}

/// `x`, `y` from the bottom-left corner, in points.
pub fn text_op(x: f32, y: f32, size: f32, bold: bool, text: &str) -> String {
    format!("BT /{} {} Tf {} {} Td {} Tj ET\n", if bold { "F2" } else { "F1" }, fmt(size), fmt(x), fmt(y), pdf_string(text))
}

const FILLER: [&str; 10] = [
    "Revenue grew steadily across all three regions during the period.",
    "Operating costs were held flat by renegotiating two supplier contracts.",
    "The northern depot moved to a four day delivery cycle in the second month.",
    "Customer support answered most requests within one working day.",
    "Two new product lines entered pilot with a small group of accounts.",
    "Inventory turnover improved after the warehouse layout was changed.",
    "Staff training hours rose, mostly in safety and equipment handling.",
    "The finance team closed each month three days earlier than last year.",
    "Energy use per unit shipped fell after the lighting was replaced.",
    "Risks for the next period are set out in the final section.",
];

/// A text document. Page `i` (1-based) carries the line `MARK-<TAG>-P<i>`.
pub fn text_document(title: &str, tag: &str, pages: usize) -> Document {
    let mut b = Builder::new();
    for i in 1..=pages {
        let mut c = String::new();
        c += &text_op(56.0, 770.0, 22.0, true, title);
        c += &text_op(56.0, 744.0, 11.0, false, &format!("Page {i} of {pages}"));
        c += &text_op(56.0, 722.0, 11.0, false, &format!("MARK-{tag}-P{i}"));
        c += "0.6 w 56 712 m 539 712 l S\n";
        for (n, line) in FILLER.iter().enumerate() {
            let which = FILLER[(n + i) % FILLER.len()];
            let _ = line;
            c += &text_op(56.0, 684.0 - n as f32 * 20.0, 11.0, false, which);
        }
        c += &text_op(56.0, 60.0, 9.0, false, &format!("{title} / {i}"));
        b.page(A4, c);
    }
    b.finish()
}

/// Names, addresses and numbers to practise redaction on.
pub fn contacts_document() -> Document {
    let mut b = Builder::new();
    let pages: [&[&str]; 2] = [
        &[
            "This agreement is made between Northfield Supply Co. and Mara Lindqvist.",
            "The Client lives at 14 Alder Row, Leeds LS6 2QT.",
            "Notices are sent to mara.lindqvist@example.com or by phone on +44 7700 900142.",
            "The Supplier may rely on instructions received from Mara Lindqvist in writing.",
            "Payments are collected from account GB29 NWBK 6016 1331 9268 19.",
        ],
        &[
            "Signed for the Client: Mara Lindqvist",
            "Signed for the Supplier: Jonas Okafor",
            "A copy was sent to accounts@example.org on 2026-10-02.",
        ],
    ];
    for (i, lines) in pages.iter().enumerate() {
        let mut c = text_op(56.0, 770.0, 20.0, true, "Services Agreement");
        c += &text_op(56.0, 744.0, 11.0, false, &format!("MARK-CONTACTS-P{}", i + 1));
        for (n, line) in lines.iter().enumerate() {
            c += &text_op(56.0, 700.0 - n as f32 * 24.0, 11.0, false, line);
        }
        b.page(A4, c);
    }
    b.finish()
}

pub const TABLE_ROWS: [[&str; 4]; 6] = [
    ["Region", "Units", "Revenue", "Margin"],
    ["North", "1204", "48160.00", "31%"],
    ["South", "980", "39200.00", "28%"],
    ["East", "1512", "60480.00", "33%"],
    ["West", "743", "29720.00", "26%"],
    ["Total", "4439", "177560.00", "30%"],
];

/// One page with a ruled table and a paragraph above it.
pub fn table_document() -> Document {
    let mut b = Builder::new();
    let mut c = text_op(56.0, 770.0, 20.0, true, "Sales by region");
    c += &text_op(56.0, 744.0, 11.0, false, "MARK-TABLE-P1");
    c += &text_op(56.0, 716.0, 11.0, false, "Figures are for the third quarter and are not audited.");
    let xs = [56.0, 196.0, 296.0, 416.0, 516.0];
    let top = 680.0;
    let row_h = 24.0;
    c += "0.5 w\n";
    for r in 0..=TABLE_ROWS.len() {
        let y = top - r as f32 * row_h;
        c += &format!("{} {} m {} {} l S\n", fmt(xs[0]), fmt(y), fmt(xs[4]), fmt(y));
    }
    for x in xs {
        c += &format!("{} {} m {} {} l S\n", fmt(x), fmt(top), fmt(x), fmt(top - TABLE_ROWS.len() as f32 * row_h));
    }
    for (r, row) in TABLE_ROWS.iter().enumerate() {
        for (col, cell) in row.iter().enumerate() {
            c += &text_op(xs[col] + 8.0, top - r as f32 * row_h - 16.0, 11.0, r == 0, cell);
        }
    }
    b.page(A4, c);
    b.finish()
}

/// Pages with different `/Rotate` values and one landscape page.
pub fn rotated_document() -> Document {
    let mut b = Builder::new();
    for (i, rot) in [0, 90, 270].iter().enumerate() {
        let mut c = text_op(56.0, 770.0, 20.0, true, &format!("Rotate {rot}"));
        c += &text_op(56.0, 744.0, 11.0, false, &format!("MARK-ROTATED-P{}", i + 1));
        let id = b.page(A4, c);
        b.doc.get_dictionary_mut(id).unwrap().set("Rotate", *rot);
    }
    let mut c = text_op(56.0, 540.0, 20.0, true, "Landscape");
    c += &text_op(56.0, 514.0, 11.0, false, "MARK-ROTATED-P4");
    b.page((842.0, 595.0), c);
    b.finish()
}

/// A deterministic image that behaves like a photograph: smooth areas plus fine noise.
pub fn photo(width: u32, height: u32, seed: u32) -> RgbImage {
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(12345);
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 24) as f32 / 255.0
    };
    RgbImage::from_fn(width, height, |x, y| {
        let fx = x as f32 / width as f32;
        let fy = y as f32 / height as f32;
        let wave = ((fx * 9.0 + seed as f32).sin() * (fy * 7.0).cos() + 1.0) / 2.0;
        let n = next() * 0.18;
        let r = (0.25 + 0.55 * fx + 0.2 * wave + n).clamp(0.0, 1.0);
        let g = (0.20 + 0.50 * fy + 0.3 * wave + n).clamp(0.0, 1.0);
        let b = (0.45 + 0.35 * (1.0 - fx) + 0.2 * (1.0 - wave) + n).clamp(0.0, 1.0);
        Rgb([(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8])
    })
}

/// Three pages, each a large losslessly stored photo. Big on purpose: a compress target.
pub fn photos_document() -> Result<Document> {
    let mut b = Builder::new();
    for i in 0..3u32 {
        let image = DynamicImage::ImageRgb8(photo(1400, 1000, i + 1));
        let id = img::add_image(&mut b.doc, &image, Encoding::Flate)?;
        let mut c = text_op(56.0, 790.0, 16.0, true, &format!("Site photo {}", i + 1));
        c += &text_op(56.0, 770.0, 11.0, false, &format!("MARK-PHOTOS-P{}", i + 1));
        c += &img::draw("Im0", 56.0, 400.0, 483.0, 345.0);
        let page = b.page(A4, c);
        doc::add_resource(&mut b.doc, page, "XObject", "Im", Object::Reference(id))?;
    }
    Ok(b.finish())
}

/// A form with two text fields and a check box, plus a free-text note and a link.
pub fn form_document() -> Document {
    let mut b = Builder::new();
    let mut c = text_op(56.0, 770.0, 20.0, true, "Equipment Loan Form");
    c += &text_op(56.0, 744.0, 11.0, false, "MARK-FORM-P1");
    c += &text_op(56.0, 700.0, 11.0, false, "Full name");
    c += &text_op(56.0, 650.0, 11.0, false, "Department");
    c += &text_op(80.0, 604.0, 11.0, false, "I have read the handling guide.");
    c += &text_op(56.0, 520.0, 11.0, false, "Borrower signature");
    c += "0.5 w 56 540 m 300 540 l S\n";
    let page = b.page(A4, c);
    let doc = &mut b.doc;
    let helv = b.regular;

    let border = |w: f32, h: f32| format!("q 1 1 1 rg 0 0 {} {} re f 0.4 w 0 G 0.5 0.5 {} {} re S Q", fmt(w), fmt(h), fmt(w - 1.0), fmt(h - 1.0)).into_bytes();
    let form = |doc: &mut Document, w: f32, h: f32, content: Vec<u8>| {
        doc.add_object(Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)], "Resources" => dictionary! { "Font" => dictionary! { "Helv" => helv } } },
            content,
        ))
    };
    let mut fields = Vec::new();
    for (name, y) in [("full_name", 676.0f32), ("department", 626.0)] {
        let ap = form(doc, 300.0, 20.0, border(300.0, 20.0));
        fields.push(doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal(name),
            "Rect" => vec![56.into(), Object::Real(y), 356.into(), Object::Real(y + 20.0)],
            "P" => page, "F" => 4, "DA" => Object::string_literal("/Helv 11 Tf 0 g"),
            "AP" => dictionary! { "N" => ap },
        }));
    }
    let on = form(doc, 14.0, 14.0, b"q 0.4 w 0 G 0.5 0.5 13 13 re S 1.5 w 3 7 m 6 3 l 11 11 l S Q".to_vec());
    let off = form(doc, 14.0, 14.0, b"q 0.4 w 0 G 0.5 0.5 13 13 re S Q".to_vec());
    fields.push(doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => Object::string_literal("agree"),
        "Rect" => vec![56.into(), 600.into(), 70.into(), 614.into()],
        "P" => page, "F" => 4, "V" => "Off", "AS" => "Off",
        "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } },
    }));
    let note_ap = form(doc, 200.0, 30.0, b"q 1 0.95 0.6 rg 0 0 200 30 re f BT /Helv 10 Tf 0 g 6 10 Td (Return by Friday) Tj ET Q".to_vec());
    let note = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FreeText", "Rect" => vec![340.into(), 700.into(), 540.into(), 730.into()],
        "Contents" => Object::string_literal("Return by Friday"), "P" => page, "F" => 4,
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "AP" => dictionary! { "N" => note_ap },
    });
    let mut annots: Vec<Object> = fields.iter().map(|id| Object::Reference(*id)).collect();
    annots.push(Object::Reference(note));
    doc.get_dictionary_mut(page).unwrap().set("Annots", annots);
    let acro = doc.add_object(dictionary! {
        "Fields" => fields.iter().map(|id| Object::Reference(*id)).collect::<Vec<_>>(),
        "DA" => Object::string_literal("/Helv 11 Tf 0 g"),
        "DR" => dictionary! { "Font" => dictionary! { "Helv" => helv } },
        "NeedAppearances" => true,
    });
    let finished = b.finish();
    let mut finished = finished;
    let catalog = finished.trailer.get(b"Root").and_then(Object::as_reference).unwrap();
    finished.get_dictionary_mut(catalog).unwrap().set("AcroForm", acro);
    finished
}

/// The report with document information and an XMP packet filled in.
/// A form with nested fields, a text field that has a value but no appearance, a radio
/// group, a combo box and a link.
pub fn choices_document() -> Document {
    let mut b = Builder::new();
    let page = b.page(A4, text_op(56.0, 770.0, 20.0, true, "Nested form") + &text_op(56.0, 744.0, 11.0, false, "MARK-NESTED-P1"));
    let d = &mut b.doc;
    let rect = |x0: i64, y0: i64, x1: i64, y1: i64| vec![Object::Integer(x0), y0.into(), x1.into(), y1.into()];
    let state = |d: &mut Document, content: &[u8]| d.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 14.into(), 14.into()] }, content.to_vec()));

    let person = d.new_object_id();
    let first = d.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "T" => Object::string_literal("first"), "Parent" => person, "Rect" => rect(56, 700, 256, 720), "P" => page, "F" => 4 });
    let last = d.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "T" => Object::string_literal("last"), "Parent" => person, "Rect" => rect(56, 660, 256, 680), "P" => page, "F" => 4, "V" => Object::string_literal("Okafor") });
    d.objects.insert(person, Object::Dictionary(dictionary! { "T" => Object::string_literal("person"), "FT" => "Tx", "DA" => Object::string_literal("/Helv 0 Tf 0 0 1 rg"), "Kids" => vec![first.into(), last.into()] }));

    let colour = d.new_object_id();
    let mut radios = Vec::new();
    for (i, name) in ["red", "blue"].iter().enumerate() {
        let on = state(d, b"q 0 g 3 3 8 8 re f Q");
        let off = state(d, b"q 0 G 0.5 0.5 13 13 re S Q");
        let mut states = Dictionary::new();
        states.set(*name, on);
        states.set("Off", off);
        radios.push(d.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => colour, "Rect" => rect(56 + 40 * i as i64, 620, 70 + 40 * i as i64, 634), "P" => page, "F" => 4, "AS" => "Off", "AP" => dictionary! { "N" => states } }));
    }
    d.objects.insert(colour, Object::Dictionary(dictionary! { "T" => Object::string_literal("colour"), "FT" => "Btn", "Ff" => 1 << 15, "V" => "Off", "Kids" => radios.iter().map(|r| Object::Reference(*r)).collect::<Vec<_>>() }));

    let size = d.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "T" => Object::string_literal("size"), "FT" => "Ch", "Ff" => 1 << 17,
        "Opt" => vec![Object::Array(vec![Object::string_literal("s"), Object::string_literal("Small")]), Object::Array(vec![Object::string_literal("l"), Object::string_literal("Large")])],
        "Rect" => rect(56, 580, 256, 600), "P" => page, "F" => 4,
    });
    let link = d.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Link", "Rect" => rect(56, 540, 256, 556), "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.com/") } });

    let mut annots: Vec<Object> = vec![first.into(), last.into()];
    annots.extend(radios.iter().map(|r| Object::Reference(*r)));
    annots.push(size.into());
    annots.push(link.into());
    if let Ok(dict) = d.get_dictionary_mut(page) {
        dict.set("Annots", annots);
    }
    let acro = d.add_object(dictionary! { "Fields" => vec![Object::Reference(person), colour.into(), size.into()], "DA" => Object::string_literal("/Helv 11 Tf 0 g") });
    let mut finished = b.finish();
    if let Ok(catalog) = finished.catalog_mut() {
        catalog.set("AcroForm", acro);
    }
    finished
}

/// A form whose "Date" field shows in three places and whose "Tick" field in two, as many
/// real forms do. The places of one field share its value.
pub fn shared_fields_document() -> Document {
    let mut b = Builder::new();
    let page = b.page(A4, text_op(56.0, 770.0, 20.0, true, "Shared fields") + &text_op(56.0, 744.0, 11.0, false, "MARK-SHARED-P1"));
    let d = &mut b.doc;
    let rect = |x0: i64, y0: i64, x1: i64, y1: i64| vec![Object::Integer(x0), y0.into(), x1.into(), y1.into()];
    let mut annots: Vec<Object> = Vec::new();
    let mut top: Vec<Object> = Vec::new();
    for (name, width, places) in [("Date", 150, vec![(56, 700), (300, 600), (300, 500)]), ("Tick", 16, vec![(56, 400), (56, 370)])] {
        let field = d.new_object_id();
        let mut kids = Vec::new();
        for (x, y) in places {
            let w = d.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Widget", "Parent" => field, "Rect" => rect(x, y, x + width, y + 18), "P" => page, "F" => 4 });
            annots.push(Object::Reference(w));
            kids.push(Object::Reference(w));
        }
        d.objects.insert(field, Object::Dictionary(dictionary! { "T" => Object::string_literal(name), "FT" => "Tx", "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "Kids" => kids }));
        top.push(Object::Reference(field));
    }
    if let Ok(dict) = d.get_dictionary_mut(page) {
        dict.set("Annots", annots);
    }
    let acro = d.add_object(dictionary! { "Fields" => top, "DA" => Object::string_literal("/Helv 10 Tf 0 g") });
    let mut finished = b.finish();
    if let Ok(catalog) = finished.catalog_mut() {
        catalog.set("AcroForm", acro);
    }
    finished
}

pub fn metadata_document() -> Document {
    let mut d = text_document("Board Minutes", "METADATA", 2);
    for (k, v) in [("Title", "Board Minutes, confidential draft"), ("Author", "Priya Raman"), ("Subject", "Internal"), ("Keywords", "minutes, draft, confidential"), ("Creator", "Acme Writer 4.2")] {
        doc::set_info(&mut d, k, v).unwrap();
    }
    let xmp = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:creator><rdf:Seq><rdf:li>Priya Raman</rdf:li></rdf:Seq></dc:creator></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#;
    let meta = d.add_object(Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, xmp.as_bytes().to_vec()).with_compression(false));
    let catalog = d.trailer.get(b"Root").and_then(Object::as_reference).unwrap();
    d.get_dictionary_mut(catalog).unwrap().set("Metadata", meta);
    d
}

/// Image-only pages made by rasterising text pages: no text layer, an OCR target.
pub fn scan_document() -> Result<Document> {
    let mut source = text_document("Delivery Note", "SCAN", 2);
    let bytes = doc::to_bytes(&mut source)?;
    let renderer = crate::render::Renderer::open(bytes, None)?;
    let (mut out, root) = doc::new_document();
    let mut pages = Vec::new();
    for i in 0..renderer.page_count() {
        let image = DynamicImage::ImageRgba8(renderer.render(i, 150.0 / 72.0)?).to_rgb8();
        let id = img::add_image(&mut out, &DynamicImage::ImageRgb8(image), Encoding::Jpeg(85))?;
        let page = doc::blank_page(&mut out, A4.0, A4.1);
        let name = doc::ensure_xobject(&mut out, page, id)?;
        doc::overlay(&mut out, page, img::draw(&name, 0.0, 0.0, A4.0, A4.1).into_bytes(), false)?;
        pages.push(page);
    }
    doc::append_pages(&mut out, root, &pages)?;
    Ok(out)
}

/// A PDF whose cross-reference table points at the wrong places.
pub fn broken_bytes() -> Result<Vec<u8>> {
    let mut d = text_document("Damaged Report", "BROKEN", 3);
    let mut out = Vec::new();
    d.save_to(&mut out).map_err(|e| Error::other(e.to_string()))?;
    // Wreck the offsets in the table and the pointer to it.
    if let Some(pos) = find_last(&out, b"startxref") {
        let end = out.len();
        for b in &mut out[pos + 9..end] {
            if b.is_ascii_digit() {
                *b = b'7';
            }
        }
    }
    if let Some(pos) = find_last(&out, b"\nxref") {
        let stop = find_last(&out, b"trailer").unwrap_or(out.len());
        for b in &mut out[pos + 5..stop] {
            if b.is_ascii_digit() {
                *b = b'0';
            }
        }
    }
    Ok(out)
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Write every fixture into `dir`. Returns the paths written.
pub fn write_all(dir: &Path) -> Result<Vec<PathBuf>> {
    crate::helpers::ensure_dir(dir)?;
    let mut out = Vec::new();
    let mut save = |name: &str, mut d: Document| -> Result<()> {
        let path = dir.join(name);
        doc::save(&mut d, &path)?;
        out.push(path);
        Ok(())
    };
    save("report.pdf", text_document("Quarterly Report", "REPORT", 6))?;
    save("letter.pdf", text_document("Cover Letter", "LETTER", 1))?;
    save("appendix.pdf", text_document("Appendix A", "APPENDIX", 3))?;
    save("contacts.pdf", contacts_document())?;
    save("table.pdf", table_document())?;
    save("rotated.pdf", rotated_document())?;
    save("form.pdf", form_document())?;
    save("choices.pdf", choices_document())?;
    save("shared.pdf", shared_fields_document())?;
    save("metadata.pdf", metadata_document())?;
    save("photos.pdf", photos_document()?)?;
    save("scan.pdf", scan_document()?)?;
    let mut v2 = Builder::new();
    for i in 1..=6 {
        let mut c = text_op(56.0, 770.0, 22.0, true, "Quarterly Report");
        c += &text_op(56.0, 744.0, 11.0, false, &format!("Page {i} of 6"));
        c += &text_op(56.0, 722.0, 11.0, false, &format!("MARK-REPORT-P{i}"));
        c += "0.6 w 56 712 m 539 712 l S\n";
        for n in 0..FILLER.len() {
            let mut line = FILLER[(n + i) % FILLER.len()].to_string();
            if i == 2 && n == 3 {
                line = "This sentence was rewritten in the second version of the report.".into();
            }
            c += &text_op(56.0, 684.0 - n as f32 * 20.0, 11.0, false, &line);
        }
        if i == 4 {
            c += &text_op(56.0, 464.0, 11.0, false, "An extra closing remark was added on page four.");
        }
        c += &text_op(56.0, 60.0, 9.0, false, &format!("Quarterly Report / {i}"));
        v2.page(A4, c);
    }
    save("report-v2.pdf", v2.finish())?;

    let broken = dir.join("broken.pdf");
    doc::write_file(&broken, &broken_bytes()?)?;
    out.push(broken);

    let images = dir.join("images");
    crate::helpers::ensure_dir(&images)?;
    let jpg = images.join("photo.jpg");
    DynamicImage::ImageRgb8(photo(1200, 800, 7)).save(&jpg)?;
    out.push(jpg);
    let png = images.join("diagram.png");
    let diagram = RgbaImage::from_fn(600, 400, |x, y| {
        let inside = (x as i32 - 300).pow(2) + (y as i32 - 200).pow(2) < 150 * 150;
        if inside { Rgba([40, 120, 200, 255]) } else if x % 40 < 2 || y % 40 < 2 { Rgba([30, 30, 30, 255]) } else { Rgba([0, 0, 0, 0]) }
    });
    diagram.save(&png)?;
    out.push(png);
    let signature = images.join("signature.png");
    let sig = RgbaImage::from_fn(400, 120, |x, y| {
        let fx = x as f32 / 400.0;
        let wave = 60.0 + 30.0 * (fx * 18.0).sin() * (1.0 - fx) + 10.0 * (fx * 5.0).cos();
        if (y as f32 - wave).abs() < 2.5 && x > 20 && x < 380 { Rgba([27, 42, 107, 255]) } else { Rgba([0, 0, 0, 0]) }
    });
    sig.save(&signature)?;
    out.push(signature);

    let html = dir.join("sample.html");
    doc::write_file(
        &html,
        b"<!doctype html><html><head><meta charset=\"utf-8\"><title>Sample page</title><style>body{font-family:sans-serif;margin:40px}h1{color:#225}</style></head><body><h1>Sample page</h1><p>MARK-HTML-P1</p><p>This page was printed to PDF from a local HTML file.</p><table border=\"1\" cellpadding=\"6\"><tr><th>Item</th><th>Qty</th></tr><tr><td>Bolts</td><td>40</td></tr></table></body></html>",
    )?;
    out.push(html);
    Ok(out)
}
