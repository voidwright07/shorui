//! A thin command line over the core, for testing and scripting.
//!
//! ```text
//! shorui-cli tools
//! shorui-cli fixtures <dir>
//! shorui-cli info <file> [--password P]
//! shorui-cli text <file> [--page N]
//! shorui-cli fields <file>
//! shorui-cli render <file> <page> <out.png> [--dpi 100]
//! shorui-cli run <tool> --out <path> [--opt '<json>'] [--password P] <inputs...>
//! ```

use shorui_core::{Ctx, doc, fixtures, render, text, tools};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn take(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    if i + 1 >= args.len() {
        return None;
    }
    args.remove(i);
    Some(args.remove(i))
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let code = match real_main(&mut args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    };
    std::process::exit(code);
}

fn real_main(args: &mut Vec<String>) -> shorui_core::Result<()> {
    let password = take(args, "--password");
    let command = if args.is_empty() { String::new() } else { args.remove(0) };
    match command.as_str() {
        "tools" => {
            for t in tools::ALL {
                println!("{:10} {:9} {}", t.id, t.group.name(), t.name);
            }
        }
        "fixtures" => {
            let dir = PathBuf::from(args.first().cloned().unwrap_or_else(|| "testdata".into()));
            for p in fixtures::write_all(&dir)? {
                println!("{}", p.display());
            }
        }
        "info" => {
            let path = PathBuf::from(args.first().ok_or_else(|| shorui_core::Error::invalid("info needs a file"))?);
            let bytes = doc::read_file(&path)?;
            let d = doc::load_bytes(&bytes, password.as_deref())?;
            let ids = doc::page_ids(&d);
            println!("pages: {}", ids.len());
            println!("bytes: {}", bytes.len());
            println!("was_encrypted: {}", d.was_encrypted());
            for (i, id) in ids.iter().enumerate().take(12) {
                let (w, h) = doc::visible_size(&d, *id);
                println!("page {}: {:.0} x {:.0} pt, rotate {}", i + 1, w, h, doc::rotation(&d, *id));
            }
        }
        "fields" => {
            let path = PathBuf::from(args.first().ok_or_else(|| shorui_core::Error::invalid("fields needs a file"))?);
            let fields = tools::edit::list_fields(&path, password.as_deref())?;
            println!("{}", serde_json::to_string_pretty(&fields).unwrap_or_default());
        }
        "text" => {
            let page = take(args, "--page").and_then(|p| p.parse::<usize>().ok());
            let path = PathBuf::from(args.first().ok_or_else(|| shorui_core::Error::invalid("text needs a file"))?);
            let reader = text::TextReader::open_path(&path, password.as_deref())?;
            let range = match page {
                Some(p) => p - 1..p,
                None => 0..reader.page_count(),
            };
            for i in range {
                println!("--- page {}", i + 1);
                println!("{}", reader.page(i)?.plain());
            }
        }
        "render" => {
            let dpi = take(args, "--dpi").and_then(|d| d.parse::<f32>().ok()).unwrap_or(100.0);
            if args.len() < 3 {
                return Err(shorui_core::Error::invalid("render needs <file> <page> <out.png>"));
            }
            let renderer = render::Renderer::open_path(&PathBuf::from(&args[0]), password.as_deref())?;
            let page: usize = args[1].parse().map_err(|_| shorui_core::Error::invalid("page must be a number"))?;
            let image = renderer.render(page - 1, render::dpi_to_scale(dpi))?;
            image.save(&args[2])?;
            println!("{} x {}", image.width(), image.height());
        }
        "run" => {
            let out = take(args, "--out").ok_or_else(|| shorui_core::Error::invalid("run needs --out <path>"))?;
            let opt = take(args, "--opt").unwrap_or_else(|| "{}".into());
            let options: serde_json::Value = serde_json::from_str(&opt).map_err(|e| shorui_core::Error::invalid(format!("--opt is not valid JSON: {e}")))?;
            if args.is_empty() {
                return Err(shorui_core::Error::invalid("run needs a tool id"));
            }
            let tool = args.remove(0);
            let inputs: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
            let cancel = AtomicBool::new(false);
            let progress = |f: f32, what: &str| eprintln!("[{:3.0}%] {what}", f * 100.0);
            let ctx = Ctx::new(&progress, &cancel).with_password(password.as_deref());
            let outcome = tools::run_json(&tool, &inputs, &PathBuf::from(out), &options, &ctx)?;
            println!("{}", serde_json::to_string_pretty(&outcome).unwrap_or_default());
        }
        _ => {
            eprintln!("usage: shorui-cli tools | fixtures <dir> | info <file> | fields <file> | text <file> | render <file> <page> <out.png> | run <tool> --out <path> [--opt JSON] <inputs...>");
        }
    }
    Ok(())
}
