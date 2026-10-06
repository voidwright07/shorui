//! The tools as the interface sees them: where they sit, what they accept, and the
//! schema that draws each tool's options panel. Option keys match `shorui_core::tools::*::Options`.

use shorui_core::tools::Group;

/// Which of the three workspace patterns a tool uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    /// A list of files.
    Queue,
    /// A grid of page thumbnails for one file.
    Grid,
    /// One page at a time with zoom.
    Canvas,
}

/// How a tool treats the files in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inputs {
    /// Each file is processed on its own and gets its own result row.
    Each,
    /// All files go into one run and one output.
    Combine,
    /// Exactly two files, old then new.
    Pair,
    /// One file at a time; the tool works on the first.
    One,
    /// Files are optional (HTML to PDF can take a URL instead).
    Optional,
}

#[derive(Debug, Clone, Copy)]
pub enum Control {
    /// Free text. `mono` for ranges, file names and anything typed precisely.
    Text { placeholder: &'static str, mono: bool },
    Password { placeholder: &'static str },
    /// A number typed into a small field.
    #[allow(dead_code)]
    Number { min: f64, max: f64, step: f64, unit: &'static str },
    Slider { min: f64, max: f64, step: f64, unit: &'static str },
    Switch,
    /// `(value, label)` pairs shown in a dropdown.
    Select(&'static [(&'static str, &'static str)]),
    /// `(value, label)` pairs shown side by side.
    Segmented(&'static [(&'static str, &'static str)]),
    /// `(value, label, description)` shown as a radio list.
    Radio(&'static [(&'static str, &'static str, &'static str)]),
    /// Multi-select chips; the value is a list of strings.
    Chips(&'static [(&'static str, &'static str)]),
    /// A colour chosen from swatches; the value is `[r, g, b]`.
    Color(&'static [(&'static str, [u8; 3])]),
    /// A path to a file chosen with the system picker. Extensions without dots.
    File { extensions: &'static [&'static str], placeholder: &'static str },
    /// Helper text, no value.
    Note,
}

#[derive(Debug, Clone, Copy)]
pub struct Field {
    /// Key in the tool's options. Empty for notes.
    pub key: &'static str,
    pub label: &'static str,
    pub control: Control,
    pub section: &'static str,
    /// Show only while another option has one of these values: `(key, values)`.
    pub show_if: Option<(&'static str, &'static [&'static str])>,
}

const fn f(section: &'static str, key: &'static str, label: &'static str, control: Control) -> Field {
    Field { key, label, control, section, show_if: None }
}
const fn fi(section: &'static str, key: &'static str, label: &'static str, control: Control, when: (&'static str, &'static [&'static str])) -> Field {
    Field { key, label, control, section, show_if: Some(when) }
}
const fn note(section: &'static str, text: &'static str) -> Field {
    Field { key: "", label: text, control: Control::Note, section, show_if: None }
}

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub group: Group,
    /// File name in `assets/icons`, without the extension.
    pub icon: &'static str,
    /// Second key of the `G` chord.
    pub chord: &'static str,
    pub workspace: Workspace,
    pub inputs: Inputs,
    /// Extensions accepted, without dots.
    pub accepts: &'static [&'static str],
    /// Appended to the input name for the output: `report` + `-compressed`.
    pub suffix: &'static str,
    /// Verb on the primary button: "Compress", "Merge".
    pub verb: &'static str,
    /// The action cannot be undone on the output (redaction).
    pub destructive: bool,
    pub fields: &'static [Field],
}

const PDF: &[&str] = &["pdf"];
const IMAGES: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff", "webp"];
const OFFICE: &[&str] = &["doc", "docx", "rtf", "odt", "txt", "xls", "xlsx", "csv", "ods", "ppt", "pptx", "odp", "pdf"];
const HTML: &[&str] = &["html", "htm"];
const CSV: &[&str] = &["csv", "tsv", "txt"];
const SIGNATURE: &[&str] = &["png", "jpg", "jpeg"];

const PAPERS: &[(&str, &str)] = &[("a4", "A4"), ("letter", "Letter"), ("a5", "A5"), ("a3", "A3"), ("legal", "Legal")];
const INKS: &[(&str, [u8; 3])] = &[("Black", [17, 17, 17]), ("Blue", [27, 42, 107]), ("Red", [122, 31, 31])];
const GREYS: &[(&str, [u8; 3])] = &[("Grey", [128, 128, 128]), ("Black", [17, 17, 17]), ("Red", [196, 55, 45]), ("Blue", [27, 42, 107])];
const PAGES: Control = Control::Text { placeholder: "all", mono: true };
const POSITIONS: &[(&str, &str)] = &[
    ("bottom-center", "Bottom centre"), ("bottom-left", "Bottom left"), ("bottom-right", "Bottom right"),
    ("top-center", "Top centre"), ("top-left", "Top left"), ("top-right", "Top right"),
];

pub const TOOLS: &[Tool] = &[
    // ----- Organise
    Tool {
        id: "merge", name: "Merge", group: Group::Organise, icon: "merge", chord: "m", workspace: Workspace::Queue, inputs: Inputs::Combine,
        accepts: PDF, suffix: "-merged", verb: "Merge", destructive: false,
        fields: &[
            f("Document", "bookmarks", "Bookmarks", Control::Select(&[("per-file", "One per file"), ("keep", "Keep existing"), ("none", "None")])),
            f("Document", "title", "Title", Control::Text { placeholder: "From first file", mono: false }),
            f("Extras", "contents_page", "Add a contents page", Control::Switch),
            f("Extras", "keep_form_fields", "Keep form fields", Control::Switch),
            f("Extras", "start_on_odd", "Start files on a right-hand page", Control::Switch),
        ],
    },
    Tool {
        id: "split", name: "Split", group: Group::Organise, icon: "split", chord: "s", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-split", verb: "Split", destructive: false,
        fields: &[
            f("Split", "mode", "Into", Control::Select(&[("each", "Single pages"), ("every", "Every N pages"), ("ranges", "Page ranges")])),
            fi("Split", "every", "Pages per file", Control::Number { min: 1.0, max: 9999.0, step: 1.0, unit: "" }, ("mode", &["every"])),
            fi("Split", "ranges", "Ranges", Control::Text { placeholder: "1-3; 4-6; 7-", mono: true }, ("mode", &["ranges"])),
            note("Split", "Separate the files with a semicolon. Each range becomes one file."),
            f("Names", "name_pattern", "File names", Control::Text { placeholder: "{name}-{n}", mono: true }),
        ],
    },
    Tool {
        id: "pages", name: "Pages", group: Group::Organise, icon: "pages", chord: "p", workspace: Workspace::Grid, inputs: Inputs::One,
        accepts: PDF, suffix: "-pages", verb: "Save pages", destructive: false,
        fields: &[],
    },
    Tool {
        id: "crop", name: "Crop & Resize", group: Group::Organise, icon: "crop", chord: "k", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-cropped", verb: "Apply", destructive: false,
        fields: &[
            f("Change", "mode", "Mode", Control::Segmented(&[("margins", "Trim"), ("auto", "Auto"), ("resize", "Resize")])),
            fi("Change", "top", "Top", Control::Number { min: -720.0, max: 720.0, step: 1.0, unit: "pt" }, ("mode", &["margins"])),
            fi("Change", "right", "Right", Control::Number { min: -720.0, max: 720.0, step: 1.0, unit: "pt" }, ("mode", &["margins"])),
            fi("Change", "bottom", "Bottom", Control::Number { min: -720.0, max: 720.0, step: 1.0, unit: "pt" }, ("mode", &["margins"])),
            fi("Change", "left", "Left", Control::Number { min: -720.0, max: 720.0, step: 1.0, unit: "pt" }, ("mode", &["margins"])),
            fi("Change", "auto_padding", "Padding", Control::Number { min: 0.0, max: 144.0, step: 1.0, unit: "pt" }, ("mode", &["auto"])),
            fi("Change", "paper", "Paper", Control::Select(PAPERS), ("mode", &["resize"])),
            fi("Change", "fit", "Content", Control::Select(&[("fit", "Fit inside"), ("fill", "Fill the page"), ("stretch", "Stretch")]), ("mode", &["resize"])),
            f("Pages", "pages", "Pages", PAGES),
        ],
    },
    Tool {
        id: "nup", name: "N-up / Booklet", group: Group::Organise, icon: "nup", chord: "n", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-nup", verb: "Impose", destructive: false,
        fields: &[
            f("Layout", "booklet", "Booklet (fold and staple)", Control::Switch),
            fi("Layout", "per_sheet", "Pages per sheet", Control::Select(&[("2", "2"), ("4", "4"), ("6", "6"), ("8", "8"), ("9", "9"), ("16", "16")]), ("booklet", &["false"])),
            fi("Layout", "order", "Order", Control::Select(&[("row", "Across, then down"), ("column", "Down, then across")]), ("booklet", &["false"])),
            f("Sheet", "paper", "Paper", Control::Select(PAPERS)),
            f("Sheet", "margin", "Margin", Control::Number { min: 0.0, max: 144.0, step: 1.0, unit: "pt" }),
            f("Sheet", "gutter", "Gap", Control::Number { min: 0.0, max: 144.0, step: 1.0, unit: "pt" }),
            f("Sheet", "border", "Outline each page", Control::Switch),
        ],
    },
    // ----- Convert
    Tool {
        id: "img2pdf", name: "Images to PDF", group: Group::Convert, icon: "img2pdf", chord: "i", workspace: Workspace::Queue, inputs: Inputs::Combine,
        accepts: IMAGES, suffix: "", verb: "Create PDF", destructive: false,
        fields: &[
            f("Page", "page_size", "Page size", Control::Select(&[("fit", "Fit the image"), ("a4", "A4"), ("letter", "Letter"), ("a5", "A5"), ("legal", "Legal")])),
            f("Page", "orientation", "Orientation", Control::Select(&[("auto", "Auto"), ("portrait", "Portrait"), ("landscape", "Landscape")])),
            f("Page", "margin", "Margin", Control::Number { min: 0.0, max: 144.0, step: 1.0, unit: "pt" }),
            f("Images", "dpi", "Resolution", Control::Number { min: 36.0, max: 1200.0, step: 1.0, unit: "dpi" }),
            note("Images", "JPEG files are stored as they are. Other formats are stored without loss."),
        ],
    },
    Tool {
        id: "pdf2img", name: "PDF to Images", group: Group::Convert, icon: "pdf2img", chord: "j", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-images", verb: "Export", destructive: false,
        fields: &[
            f("Images", "format", "Format", Control::Segmented(&[("png", "PNG"), ("jpg", "JPEG")])),
            f("Images", "dpi", "Resolution", Control::Select(&[("72", "72 dpi"), ("96", "96 dpi"), ("150", "150 dpi"), ("200", "200 dpi"), ("300", "300 dpi"), ("600", "600 dpi")])),
            fi("Images", "quality", "Quality", Control::Slider { min: 10.0, max: 100.0, step: 1.0, unit: "" }, ("format", &["jpg"])),
            f("Pages", "pages", "Pages", PAGES),
        ],
    },
    Tool {
        id: "office", name: "Office to/from PDF", group: Group::Convert, icon: "office", chord: "o", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: OFFICE, suffix: "", verb: "Convert", destructive: false,
        fields: &[
            f("Engine", "engine", "Convert with", Control::Select(&[("auto", "Automatic"), ("libreoffice", "LibreOffice"), ("msoffice", "Microsoft Office")])),
            note("Engine", "Word, Excel and PowerPoint files become PDF using LibreOffice or Microsoft Office on this machine. A PDF becomes a Word document: with LibreOffice if it is installed, otherwise as text only. Choosing Microsoft Office gives a closer copy, and Word shows itself briefly while it works."),
        ],
    },
    Tool {
        id: "html", name: "HTML/URL to PDF", group: Group::Convert, icon: "html", chord: "h", workspace: Workspace::Queue, inputs: Inputs::Optional,
        accepts: HTML, suffix: "", verb: "Print to PDF", destructive: false,
        fields: &[
            f("Source", "url", "Web address", Control::Text { placeholder: "https://", mono: true }),
            note("Source", "Leave empty to print the HTML files in the queue. A web address is the one thing in Shorui that uses the network."),
            f("Page", "paper", "Paper", Control::Select(&[("a4", "A4"), ("letter", "Letter"), ("a3", "A3"), ("legal", "Legal")])),
            f("Page", "landscape", "Landscape", Control::Switch),
            f("Page", "margins", "Margins", Control::Switch),
            f("Page", "background", "Print backgrounds", Control::Switch),
        ],
    },
    Tool {
        id: "csv2pdf", name: "CSV to PDF", group: Group::Convert, icon: "csv2pdf", chord: "g", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: CSV, suffix: "", verb: "Create PDF", destructive: false,
        fields: &[
            f("Data", "delimiter", "Separator", Control::Select(&[("auto", "Detect"), ("comma", "Comma"), ("semicolon", "Semicolon"), ("tab", "Tab"), ("pipe", "Pipe")])),
            f("Data", "header", "First row is a header", Control::Switch),
            f("Page", "paper", "Paper", Control::Select(&[("a4", "A4"), ("letter", "Letter"), ("a3", "A3"), ("legal", "Legal")])),
            f("Page", "orientation", "Orientation", Control::Select(&[("auto", "Auto"), ("portrait", "Portrait"), ("landscape", "Landscape")])),
            f("Page", "margin", "Margin", Control::Number { min: 0.0, max: 144.0, step: 1.0, unit: "pt" }),
            f("Table", "font_size", "Text size", Control::Number { min: 5.0, max: 72.0, step: 0.5, unit: "pt" }),
            f("Table", "grid", "Cell borders", Control::Switch),
            f("Table", "stripes", "Shade every other row", Control::Switch),
            f("Table", "page_numbers", "Page numbers", Control::Switch),
            note("Table", "Wide tables wrap long cells and turn landscape when needed. Columns of numbers are right-aligned."),
        ],
    },
    Tool {
        id: "pdfa", name: "PDF/A", group: Group::Convert, icon: "pdfa", chord: "a", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-pdfa", verb: "Convert", destructive: false,
        fields: &[
            f("Archive", "mode", "Method", Control::Radio(&[
                ("auto", "Automatic", "Keeps text when every font is embedded"),
                ("preserve", "Keep text", "Pages stay as they are"),
                ("image", "As images", "Always conforms, text is not selectable"),
            ])),
            note("Archive", "Targets PDF/A-2b. Shorui does not run a full PDF/A validator."),
        ],
    },
    Tool {
        id: "tables", name: "Extract Tables", group: Group::Convert, icon: "tables", chord: "t", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-tables", verb: "Extract", destructive: false,
        fields: &[
            f("Tables", "pages", "Pages", PAGES),
            f("Tables", "delimiter", "Separator", Control::Select(&[("comma", "Comma"), ("semicolon", "Semicolon"), ("tab", "Tab")])),
            f("Tables", "single_file", "One CSV for everything", Control::Switch),
        ],
    },
    // ----- Edit
    Tool {
        id: "edit", name: "Edit & Fill", group: Group::Edit, icon: "edit", chord: "e", workspace: Workspace::Canvas, inputs: Inputs::One,
        accepts: PDF, suffix: "-filled", verb: "Save copy", destructive: false,
        fields: &[
            f("When saving", "flatten", "Flatten filled fields", Control::Switch),
            note("When saving", "Flattening turns entries into fixed page content that can no longer be edited."),
        ],
    },
    Tool {
        id: "sign", name: "Sign", group: Group::Edit, icon: "sign", chord: "y", workspace: Workspace::Canvas, inputs: Inputs::One,
        accepts: PDF, suffix: "-signed", verb: "Save signed copy", destructive: false,
        fields: &[
            f("Signature", "image", "Image", Control::File { extensions: SIGNATURE, placeholder: "Choose a PNG or JPEG" }),
            f("Signature", "typed", "Or type it", Control::Text { placeholder: "Your name", mono: false }),
            f("Signature", "width", "Size", Control::Slider { min: 40.0, max: 400.0, step: 1.0, unit: "pt" }),
            f("Signature", "color", "Ink", Control::Color(INKS)),
            f("Signature", "add_date", "Add date beside it", Control::Switch),
            f("Signature", "all_pages", "Sign every page", Control::Switch),
            note("Signature", "This places a picture of a signature. It is not a cryptographic signature."),
        ],
    },
    Tool {
        id: "watermark", name: "Watermark", group: Group::Edit, icon: "watermark", chord: "w", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-watermarked", verb: "Stamp", destructive: false,
        fields: &[
            f("Mark", "text", "Text", Control::Text { placeholder: "CONFIDENTIAL", mono: false }),
            f("Mark", "image", "Or an image", Control::File { extensions: SIGNATURE, placeholder: "Choose a PNG or JPEG" }),
            f("Mark", "position", "Position", Control::Select(&[("center", "Centre"), ("tile", "Tiled"), ("top-left", "Top left"), ("top-right", "Top right"), ("bottom-left", "Bottom left"), ("bottom-right", "Bottom right")])),
            f("Look", "opacity", "Opacity", Control::Slider { min: 0.05, max: 1.0, step: 0.05, unit: "" }),
            f("Look", "rotation", "Angle", Control::Slider { min: -90.0, max: 90.0, step: 5.0, unit: "°" }),
            f("Look", "color", "Colour", Control::Color(GREYS)),
            f("Look", "under", "Behind the content", Control::Switch),
            f("Pages", "pages", "Pages", PAGES),
        ],
    },
    Tool {
        id: "numbers", name: "Page Numbers & Bates", group: Group::Edit, icon: "numbers", chord: "b", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-numbered", verb: "Add", destructive: false,
        fields: &[
            f("Add", "mode", "Kind", Control::Segmented(&[("page-numbers", "Numbers"), ("bates", "Bates"), ("header-footer", "Header")])),
            fi("Add", "format", "Format", Control::Text { placeholder: "Page {n} of {total}", mono: true }, ("mode", &["page-numbers"])),
            fi("Add", "start", "Start at", Control::Number { min: 0.0, max: 999999.0, step: 1.0, unit: "" }, ("mode", &["page-numbers"])),
            fi("Add", "bates_prefix", "Prefix", Control::Text { placeholder: "ABC", mono: true }, ("mode", &["bates"])),
            fi("Add", "bates_digits", "Digits", Control::Number { min: 1.0, max: 12.0, step: 1.0, unit: "" }, ("mode", &["bates"])),
            fi("Add", "bates_start", "Start at", Control::Number { min: 0.0, max: 999999999.0, step: 1.0, unit: "" }, ("mode", &["bates"])),
            fi("Add", "header_left", "Header left", Control::Text { placeholder: "{file}", mono: true }, ("mode", &["header-footer"])),
            fi("Add", "header_center", "Header centre", Control::Text { placeholder: "", mono: true }, ("mode", &["header-footer"])),
            fi("Add", "header_right", "Header right", Control::Text { placeholder: "{date}", mono: true }, ("mode", &["header-footer"])),
            fi("Add", "footer_left", "Footer left", Control::Text { placeholder: "", mono: true }, ("mode", &["header-footer"])),
            fi("Add", "footer_center", "Footer centre", Control::Text { placeholder: "{n} / {total}", mono: true }, ("mode", &["header-footer"])),
            fi("Add", "footer_right", "Footer right", Control::Text { placeholder: "", mono: true }, ("mode", &["header-footer"])),
            fi("Place", "position", "Position", Control::Select(POSITIONS), ("mode", &["page-numbers", "bates"])),
            f("Place", "size", "Text size", Control::Number { min: 5.0, max: 48.0, step: 0.5, unit: "pt" }),
            f("Place", "margin", "From the edge", Control::Number { min: 0.0, max: 144.0, step: 1.0, unit: "pt" }),
            f("Place", "skip_first", "Skip the first page", Control::Switch),
            f("Pages", "pages", "Pages", PAGES),
        ],
    },
    Tool {
        id: "flatten", name: "Flatten", group: Group::Edit, icon: "flatten", chord: "f", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-flattened", verb: "Flatten", destructive: false,
        fields: &[
            f("Flatten", "forms", "Form fields", Control::Switch),
            f("Flatten", "annotations", "Comments and markup", Control::Switch),
            f("Flatten", "keep_links", "Keep links clickable", Control::Switch),
        ],
    },
    // ----- Secure
    Tool {
        id: "protect", name: "Protect", group: Group::Secure, icon: "protect", chord: "l", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-protected", verb: "Protect", destructive: false,
        fields: &[
            f("Password", "user_password", "To open", Control::Password { placeholder: "Password" }),
            f("Password", "owner_password", "To change", Control::Password { placeholder: "Same as above" }),
            f("Password", "encryption", "Strength", Control::Select(&[("aes-256", "AES 256"), ("aes-128", "AES 128")])),
            f("Allow", "allow_print", "Printing", Control::Switch),
            f("Allow", "allow_copy", "Copying text", Control::Switch),
            f("Allow", "allow_modify", "Editing", Control::Switch),
            f("Allow", "allow_annotate", "Commenting", Control::Switch),
            f("Allow", "allow_forms", "Filling forms", Control::Switch),
        ],
    },
    Tool {
        id: "unlock", name: "Unlock", group: Group::Secure, icon: "unlock", chord: "u", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-unlocked", verb: "Unlock", destructive: false,
        fields: &[
            f("Password", "@password", "Password", Control::Password { placeholder: "The file's password" }),
            note("Password", "You need the password to remove it. Shorui cannot open a file without it."),
        ],
    },
    Tool {
        id: "redact", name: "Redact", group: Group::Secure, icon: "redact", chord: "r", workspace: Workspace::Canvas, inputs: Inputs::One,
        accepts: PDF, suffix: "-redacted", verb: "Apply redactions", destructive: true,
        fields: &[
            f("Find patterns", "patterns", "", Control::Chips(&[("email", "Email"), ("phone", "Phone"), ("iban", "IBAN"), ("date", "Dates"), ("card", "Card numbers")])),
            f("When applying", "strip_metadata", "Also strip metadata", Control::Switch),
            f("When applying", "dpi", "Page quality", Control::Select(&[("150", "150 dpi"), ("200", "200 dpi"), ("300", "300 dpi")])),
        ],
    },
    Tool {
        id: "strip", name: "Strip Metadata", group: Group::Secure, icon: "strip", chord: "x", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-clean", verb: "Strip", destructive: false,
        fields: &[
            f("Remove", "info", "Title, author and dates", Control::Switch),
            f("Remove", "xmp", "XMP metadata", Control::Switch),
            f("Remove", "annotations_meta", "Names on comments", Control::Switch),
            f("Remove", "attachments", "Attached files", Control::Switch),
            f("Remove", "javascript", "Scripts", Control::Switch),
            f("Remove", "thumbnails", "Embedded thumbnails", Control::Switch),
            f("Remove", "private_data", "Application data", Control::Switch),
        ],
    },
    // ----- Optimise
    Tool {
        id: "compress", name: "Compress", group: Group::Optimise, icon: "compress", chord: "c", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-compressed", verb: "Compress", destructive: false,
        fields: &[
            f("Quality preset", "preset", "", Control::Radio(&[
                ("light", "Light", "Keeps print quality"),
                ("balanced", "Balanced", "Screen and email"),
                ("strong", "Strong", "Smallest that stays readable"),
                ("custom", "Custom", "Set each option yourself"),
            ])),
            fi("Images", "image_quality", "Quality", Control::Slider { min: 10.0, max: 95.0, step: 1.0, unit: "" }, ("preset", &["custom"])),
            fi("Images", "max_dpi", "Downsample above", Control::Select(&[("72", "72 dpi"), ("96", "96 dpi"), ("150", "150 dpi"), ("220", "220 dpi"), ("300", "300 dpi")]), ("preset", &["custom"])),
            f("Also", "grayscale", "Greyscale images", Control::Switch),
            f("Also", "strip_metadata", "Remove metadata", Control::Switch),
        ],
    },
    Tool {
        id: "ocr", name: "OCR", group: Group::Optimise, icon: "ocr", chord: "q", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-searchable", verb: "Recognise text", destructive: false,
        fields: &[
            f("Recognition", "language", "Language", Control::Text { placeholder: "System default", mono: true }),
            f("Recognition", "dpi", "Read at", Control::Select(&[("150", "150 dpi"), ("200", "200 dpi"), ("300", "300 dpi")])),
            f("Recognition", "skip_text_pages", "Skip pages that have text", Control::Switch),
            f("Pages", "pages", "Pages", PAGES),
            note("Pages", "Adds an invisible, selectable text layer. The page image is not changed."),
        ],
    },
    Tool {
        id: "repair", name: "Repair", group: Group::Optimise, icon: "repair", chord: "v", workspace: Workspace::Queue, inputs: Inputs::Each,
        accepts: PDF, suffix: "-repaired", verb: "Repair", destructive: false,
        fields: &[note("Repair", "Rebuilds the page index and recovers every page that can still be read. The original file is not changed.")],
    },
    Tool {
        id: "compare", name: "Compare", group: Group::Optimise, icon: "compare", chord: "d", workspace: Workspace::Queue, inputs: Inputs::Pair,
        accepts: PDF, suffix: "-comparison", verb: "Compare", destructive: false,
        fields: &[
            f("Compare", "mode", "Look at", Control::Segmented(&[("both", "Both"), ("text", "Text"), ("visual", "Pixels")])),
            f("Compare", "dpi", "Resolution", Control::Select(&[("72", "72 dpi"), ("100", "100 dpi"), ("150", "150 dpi")])),
            note("Compare", "The first file in the queue is the old version, the second is the new one."),
        ],
    },
];

pub fn tool(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

pub fn in_group(group: Group) -> impl Iterator<Item = &'static Tool> {
    TOOLS.iter().filter(move |t| t.group == group)
}

/// Tools worth offering for a file with this extension, best first.
pub fn suggested_for(ext: &str) -> &'static [&'static str] {
    match ext {
        "pdf" => &["compress", "split", "ocr", "pages", "protect"],
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tif" | "tiff" | "webp" => &["img2pdf"],
        "html" | "htm" => &["html"],
        "csv" | "tsv" => &["csv2pdf", "office"],
        "doc" | "docx" | "rtf" | "odt" | "txt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" => &["office"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalogue_matches_core() {
        assert_eq!(TOOLS.len(), 25);
        assert_eq!(TOOLS.len(), shorui_core::tools::ALL.len());
        let ids: HashSet<&str> = TOOLS.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), 25);
        for info in shorui_core::tools::ALL {
            let t = tool(info.id).unwrap_or_else(|| panic!("{} missing from the catalogue", info.id));
            assert_eq!(t.group, info.group, "{}", info.id);
            assert_eq!(t.name, info.name, "{}", info.id);
        }
        let chords: HashSet<&str> = TOOLS.iter().map(|t| t.chord).collect();
        assert_eq!(chords.len(), 25, "chords must be unique");
    }

    /// The panel can only bind to options the tool really has, with a matching type.
    #[test]
    fn schema_keys_exist_in_core_options() {
        let mut problems = Vec::new();
        for t in TOOLS {
            let Some(serde_json::Value::Object(defaults)) = shorui_core::tools::default_options(t.id) else {
                problems.push(format!("{}: no default options", t.id));
                continue;
            };
            for f in t.fields.iter().filter(|f| !f.key.is_empty() && !f.key.starts_with('@')) {
                match defaults.get(f.key) {
                    None => problems.push(format!("{}.{}: not an option of the tool (has: {:?})", t.id, f.key, defaults.keys().collect::<Vec<_>>())),
                    Some(v) => {
                        let ok = match f.control {
                            Control::Switch => v.is_boolean(),
                            Control::Number { .. } => v.is_number() || v.is_null(),
                            Control::Slider { .. } => v.is_number(),
                            Control::Chips(_) | Control::Color(_) => v.is_array(),
                            Control::Select(options) | Control::Segmented(options) => {
                                let text = crate::state::value_text(v);
                                options.iter().any(|(value, _)| *value == text)
                            }
                            Control::Radio(options) => {
                                let text = crate::state::value_text(v);
                                options.iter().any(|(value, _, _)| *value == text)
                            }
                            Control::Text { .. } | Control::Password { .. } | Control::File { .. } => v.is_string() || v.is_null(),
                            Control::Note => true,
                        };
                        if !ok {
                            problems.push(format!("{}.{}: default {v} does not fit the control", t.id, f.key));
                        }
                    }
                }
            }
        }
        assert!(problems.is_empty(), "
{}", problems.join("
"));
    }

    #[test]
    fn every_tool_has_an_icon_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
        for t in TOOLS {
            assert!(dir.join(format!("{}.svg", t.icon)).is_file(), "missing icon {}", t.icon);
        }
    }
}
