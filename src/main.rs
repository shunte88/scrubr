mod optimizer;
mod path;
mod css;
mod color;
mod ids;
mod transform;

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use clap::{Arg, ArgAction, Command};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use optimizer::{ScrubrOptions, optimize_svg};

fn main() {
    let matches = Command::new("scrubr")
        .version("1.0.0")
        .about("SVG Optimizer / Cleaner")
        .arg(Arg::new("input")
            .short('i')
            .long("input")
            .value_name("INPUT.SVG")
            .help("Input SVG file (default: stdin)"))
        .arg(Arg::new("output")
            .short('o')
            .long("output")
            .value_name("OUTPUT.SVG")
            .help("Output SVG file (default: stdout)"))
        .arg(Arg::new("quiet")
            .short('q')
            .long("quiet")
            .action(ArgAction::SetTrue)
            .help("Suppress non-error output"))
        .arg(Arg::new("verbose")
            .short('v')
            .long("verbose")
            .action(ArgAction::SetTrue)
            .help("Verbose output (statistics, etc.)"))
        // Optimization
        .arg(Arg::new("set-precision")
            .long("set-precision")
            .value_name("NUM")
            .default_value("5")
            .help("Set number of significant digits (default: 5)"))
        .arg(Arg::new("set-c-precision")
            .long("set-c-precision")
            .value_name("NUM")
            .help("Set number of significant digits for control points"))
        .arg(Arg::new("disable-simplify-colors")
            .long("disable-simplify-colors")
            .action(ArgAction::SetTrue)
            .help("Won't convert colors to #RRGGBB format"))
        .arg(Arg::new("disable-style-to-xml")
            .long("disable-style-to-xml")
            .action(ArgAction::SetTrue)
            .help("Won't convert styles into XML attributes"))
        .arg(Arg::new("disable-group-collapsing")
            .long("disable-group-collapsing")
            .action(ArgAction::SetTrue)
            .help("Won't collapse <g> elements"))
        .arg(Arg::new("create-groups")
            .long("create-groups")
            .action(ArgAction::SetTrue)
            .help("Create <g> elements for runs of elements with identical attributes"))
        .arg(Arg::new("keep-editor-data")
            .long("keep-editor-data")
            .action(ArgAction::SetTrue)
            .help("Won't remove Inkscape, Sodipodi, Adobe Illustrator or Sketch elements/attributes"))
        .arg(Arg::new("keep-unreferenced-defs")
            .long("keep-unreferenced-defs")
            .action(ArgAction::SetTrue)
            .help("Won't remove elements within <defs> that are unreferenced"))
        .arg(Arg::new("renderer-workaround")
            .long("renderer-workaround")
            .action(ArgAction::SetTrue)
            .help("Work around various renderer bugs (default)"))
        .arg(Arg::new("no-renderer-workaround")
            .long("no-renderer-workaround")
            .action(ArgAction::SetTrue)
            .help("Do not work around renderer bugs"))
        // SVG document
        .arg(Arg::new("strip-xml-prolog")
            .long("strip-xml-prolog")
            .action(ArgAction::SetTrue)
            .help("Won't output the XML prolog (<?xml ?>)"))
        .arg(Arg::new("remove-titles")
            .long("remove-titles")
            .action(ArgAction::SetTrue)
            .help("Remove <title> elements"))
        .arg(Arg::new("remove-descriptions")
            .long("remove-descriptions")
            .action(ArgAction::SetTrue)
            .help("Remove <desc> elements"))
        .arg(Arg::new("remove-metadata")
            .long("remove-metadata")
            .action(ArgAction::SetTrue)
            .help("Remove <metadata> elements"))
        .arg(Arg::new("remove-descriptive-elements")
            .long("remove-descriptive-elements")
            .action(ArgAction::SetTrue)
            .help("Remove <title>, <desc> and <metadata> elements"))
        .arg(Arg::new("enable-comment-stripping")
            .long("enable-comment-stripping")
            .action(ArgAction::SetTrue)
            .help("Remove all comments (<!-- -->)"))
        .arg(Arg::new("disable-embed-rasters")
            .long("disable-embed-rasters")
            .action(ArgAction::SetTrue)
            .help("Won't embed rasters as base64-encoded data"))
        .arg(Arg::new("enable-viewboxing")
            .long("enable-viewboxing")
            .action(ArgAction::SetTrue)
            .help("Changes document width/height to 100%/100% and creates viewBox"))
        // Output formatting
        .arg(Arg::new("indent")
            .long("indent")
            .value_name("TYPE")
            .default_value("space")
            .help("Indentation type: none, space, tab (default: space)"))
        .arg(Arg::new("nindent")
            .long("nindent")
            .value_name("NUM")
            .default_value("1")
            .help("Depth of indentation (default: 1)"))
        .arg(Arg::new("no-line-breaks")
            .long("no-line-breaks")
            .action(ArgAction::SetTrue)
            .help("Do not create line breaks in output"))
        .arg(Arg::new("strip-xml-space")
            .long("strip-xml-space")
            .action(ArgAction::SetTrue)
            .help("Strip xml:space=\"preserve\" from root SVG element"))
        // ID attributes
        .arg(Arg::new("enable-id-stripping")
            .long("enable-id-stripping")
            .action(ArgAction::SetTrue)
            .help("Remove all unreferenced IDs"))
        .arg(Arg::new("shorten-ids")
            .long("shorten-ids")
            .action(ArgAction::SetTrue)
            .help("Shorten all IDs to the least number of letters possible"))
        .arg(Arg::new("shorten-ids-prefix")
            .long("shorten-ids-prefix")
            .value_name("PREFIX")
            .help("Add custom prefix to shortened IDs"))
        .arg(Arg::new("protect-ids-noninkscape")
            .long("protect-ids-noninkscape")
            .action(ArgAction::SetTrue)
            .help("Don't remove IDs not ending with a digit"))
        .arg(Arg::new("protect-ids-list")
            .long("protect-ids-list")
            .value_name("LIST")
            .help("Comma-separated list of IDs to protect"))
        .arg(Arg::new("protect-ids-prefix")
            .long("protect-ids-prefix")
            .value_name("PREFIX")
            .help("Don't remove IDs starting with given prefix"))
        // Compat
        .arg(Arg::new("error-on-flowtext")
            .long("error-on-flowtext")
            .action(ArgAction::SetTrue)
            .help("Exit with error if the input SVG uses nonstandard flowing text"))
        .get_matches();

    let precision: u8 = matches.get_one::<String>("set-precision")
        .unwrap().parse().unwrap_or(5);
    let c_precision: u8 = matches.get_one::<String>("set-c-precision")
        .map(|s| s.parse().unwrap_or(precision))
        .unwrap_or(precision);

    let protect_ids_list: Vec<String> = matches.get_one::<String>("protect-ids-list")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();

    let indent_str = matches.get_one::<String>("indent").unwrap().as_str();
    let indent = match indent_str {
        "none" => optimizer::Indent::None,
        "tab"  => optimizer::Indent::Tab,
        _      => optimizer::Indent::Space,
    };
    let nindent: u8 = matches.get_one::<String>("nindent")
        .unwrap().parse().unwrap_or(1);

    let renderer_workaround = !matches.get_flag("no-renderer-workaround");

    let options = ScrubrOptions {
        precision,
        c_precision,
        simplify_colors: !matches.get_flag("disable-simplify-colors"),
        style_to_xml:    !matches.get_flag("disable-style-to-xml"),
        group_collapsing: !matches.get_flag("disable-group-collapsing"),
        create_groups:   matches.get_flag("create-groups"),
        keep_editor_data: matches.get_flag("keep-editor-data"),
        keep_unreferenced_defs: matches.get_flag("keep-unreferenced-defs"),
        renderer_workaround,
        strip_xml_prolog: matches.get_flag("strip-xml-prolog"),
        remove_titles:   matches.get_flag("remove-titles")
            || matches.get_flag("remove-descriptive-elements"),
        remove_descriptions: matches.get_flag("remove-descriptions")
            || matches.get_flag("remove-descriptive-elements"),
        remove_metadata: matches.get_flag("remove-metadata")
            || matches.get_flag("remove-descriptive-elements"),
        strip_comments:  matches.get_flag("enable-comment-stripping"),
        embed_rasters:   !matches.get_flag("disable-embed-rasters"),
        enable_viewboxing: matches.get_flag("enable-viewboxing"),
        indent,
        nindent,
        no_line_breaks:  matches.get_flag("no-line-breaks"),
        strip_xml_space: matches.get_flag("strip-xml-space"),
        strip_ids:       matches.get_flag("enable-id-stripping"),
        shorten_ids:     matches.get_flag("shorten-ids"),
        shorten_ids_prefix: matches.get_one::<String>("shorten-ids-prefix").cloned(),
        protect_ids_noninkscape: matches.get_flag("protect-ids-noninkscape"),
        protect_ids_list,
        protect_ids_prefix: matches.get_one::<String>("protect-ids-prefix").cloned(),
        error_on_flowtext: matches.get_flag("error-on-flowtext"),
        quiet:  matches.get_flag("quiet"),
        verbose: matches.get_flag("verbose"),
    };

    // Read input
    let input_path = matches.get_one::<String>("input");
    let output_path = matches.get_one::<String>("output");

    let raw_bytes: Vec<u8> = match input_path {
        Some(p) => {
            fs::read(p).unwrap_or_else(|e| {
                eprintln!("Error reading input file '{}': {}", p, e);
                std::process::exit(1);
            })
        }
        None => {
            let mut buf = Vec::new();
            io::stdin().read_to_end(&mut buf).unwrap_or_else(|e| {
                eprintln!("Error reading stdin: {}", e);
                std::process::exit(1);
            });
            buf
        }
    };

    // Detect SVGZ (gzip) by magic bytes
    let is_svgz_in = raw_bytes.starts_with(&[0x1f, 0x8b]);
    let svg_text = if is_svgz_in {
        let mut d = GzDecoder::new(&raw_bytes[..]);
        let mut s = String::new();
        d.read_to_string(&mut s).unwrap_or_else(|e| {
            eprintln!("Error decompressing SVGZ: {}", e);
            std::process::exit(1);
        });
        s
    } else {
        String::from_utf8_lossy(&raw_bytes).into_owned()
    };

    // Detect output format
    let is_svgz_out = output_path.as_ref()
        .map(|p| p.to_lowercase().ends_with(".svgz"))
        .unwrap_or(is_svgz_in);

    // Run optimizer
    let (optimized, stats) = optimize_svg(&svg_text, &options);

    if options.error_on_flowtext && stats.has_flowtext {
        eprintln!("Error: input SVG uses nonstandard flowing text (flowRoot/flowPara).");
        std::process::exit(1);
    } else if stats.has_flowtext && !options.quiet {
        eprintln!("Warning: input SVG uses nonstandard flowing text (flowRoot/flowPara).");
    }

    if options.verbose && !options.quiet {
        let orig = svg_text.len();
        let out  = optimized.len();
        let pct  = if orig > 0 { 100.0 * (1.0 - out as f64 / orig as f64) } else { 0.0 };
        eprintln!("Original:  {} bytes", orig);
        eprintln!("Optimized: {} bytes", out);
        eprintln!("Reduction: {:.2}%", pct);
        if stats.subst_vars_preserved > 0 {
            eprintln!("Substitution variables preserved: {}", stats.subst_vars_preserved);
        }
    }

    // Write output
    let out_bytes: Vec<u8> = if is_svgz_out {
        let mut enc = GzEncoder::new(Vec::new(), Compression::best());
        enc.write_all(optimized.as_bytes()).unwrap();
        enc.finish().unwrap()
    } else {
        optimized.into_bytes()
    };

    match output_path {
        Some(p) => {
            // Create parent dirs if needed
            if let Some(parent) = Path::new(p).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(parent).ok();
                }
            }
            fs::write(p, &out_bytes).unwrap_or_else(|e| {
                eprintln!("Error writing output file '{}': {}", p, e);
                std::process::exit(1);
            });
        }
        None => {
            io::stdout().write_all(&out_bytes).unwrap_or_else(|e| {
                eprintln!("Error writing stdout: {}", e);
                std::process::exit(1);
            });
        }
    }
}
