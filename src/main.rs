use chrono::NaiveDate;
use csv::WriterBuilder;
use image::imageops::resize;
use image::GrayImage;
use imageproc::integral_image::{integral_squared_image, sum_image_pixels};
use leptess::leptonica::Box as LepBox;
use leptess::LepTess;
use once_cell::sync::Lazy;
use regex::Regex;
use rustfft::num_complex::Complex;
use rustfft::{FftDirection, FftPlanner};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

#[derive(Clone)]
struct BillRecord {
    filename: String,
    date: Option<String>,
    tariff: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
    amount: Option<String>,
    bill_type: String,
    account_number: Option<String>,
    meter_number: Option<String>,
    address: Option<String>,
    fingerprint: String,
}

struct DuplicateEntry {
    files: Vec<String>,
    match_type: String,
    fingerprint: Option<String>,
    date: Option<String>,
    amount: Option<String>,
    bill_type: Option<String>,
}

struct TemplateImage {
    name: String,
    image: GrayImage,
}

struct PreparedTemplate {
    name: String,
    scaled: Vec<GrayImage>,
}

#[derive(Clone)]
struct TemplateMatch {
    template_name: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    score: f32,
}

struct RunOptions {
    input_dir: PathBuf,
    template_dir: PathBuf,
    match_threshold: f32,
    render_dpi: u32,
}

static DATE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)(\d{1,2}[/-]\d{1,2}[/-]\d{2,4})").unwrap(),
        Regex::new(r"(?i)(\d{1,2}\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{2,4})").unwrap(),
        Regex::new(r"(?i)((?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+\d{2,4})").unwrap(),
        Regex::new(r"(?i)(?:\(|-)?\s*(\d{1,2}(?:st|nd|rd|th)?\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{4})\s*(?:\)|-)?").unwrap(),
    ]
});

static AMOUNT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)[$\u{00A3}\u{20AC}](\d+\.\d{2})").unwrap(),
        Regex::new(r"(?i)\u{00A3}(\d+\.\d{2})").unwrap(),
        Regex::new(r"(?i)(\d+\.\d{2})[$\u{00A3}\u{20AC}]").unwrap(),
        Regex::new(r"(?i)Total:?\s*[$\u{00A3}\u{20AC}]?(\d+\.\d{2})").unwrap(),
        Regex::new(r"(?i)Amount\s*due:?\s*[$\u{00A3}\u{20AC}]?(\d+\.\d{2})").unwrap(),
        Regex::new(
            r"(?i)Total\s+(?:Electricity|Gas)?\s+Charges\s*[$\u{00A3}\u{20AC}]?(\d+\.\d{2})",
        )
        .unwrap(),
        Regex::new(r"(?i)Total\s+charges\s+for\s+bill\s*[$\u{00A3}\u{20AC}]?(\d+\.\d{2})").unwrap(),
    ]
});

static ACCOUNT_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)Account\s*(?:Number|No|#)?\s*:?\s*(\d+[-\s]?\d+)").unwrap(),
        Regex::new(r"(?i)Account\s*(?:Number|No|#)?\s*:?\s*([A-Z0-9]+)").unwrap(),
        Regex::new(r"(?i)Supply\s+number\s*:?\s*([A-Z0-9]+)").unwrap(),
    ]
});

static METER_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?i)Meter\s+(?:Number|No|#)?\s*:?\s*([A-Z0-9]+)").unwrap(),
        Regex::new(r"(?i)(?:for|from)\s+Meter\s+([A-Z0-9]+)").unwrap(),
    ]
});

static ADDRESS_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"(?is)Supply\s+Address:?\s*(.*?)(?:Postcode|$)").unwrap(),
        Regex::new(r"(?is)Address:?\s*(.*?)(?:Postcode|$)").unwrap(),
    ]
});

static GAS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)gas").unwrap());
static ELECTRIC_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)electric|electricity").unwrap());
static WHITESPACE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static ORDINAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{1,2})(st|nd|rd|th)").unwrap());

fn print_usage() {
    println!(
        "Usage: bill_analyzer [--input DIR] [--templates DIR] [--match-threshold VALUE] [--dpi VALUE]"
    );
}

fn parse_args(current_dir: &Path) -> Result<RunOptions, Box<dyn Error>> {
    let mut input_dir = current_dir.to_path_buf();
    let mut template_dir = current_dir.to_path_buf();
    let mut match_threshold = 0.72_f32;
    let mut render_dpi = 180_u32;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" => {
                let value = args.next().ok_or("--input requires a directory path")?;
                input_dir = PathBuf::from(value);
            }
            "--templates" => {
                let value = args.next().ok_or("--templates requires a directory path")?;
                template_dir = PathBuf::from(value);
            }
            "--match-threshold" => {
                let value = args.next().ok_or("--match-threshold requires a value")?;
                match_threshold = value.parse::<f32>()?;
            }
            "--dpi" => {
                let value = args.next().ok_or("--dpi requires a value")?;
                render_dpi = value.parse::<u32>()?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ => {
                return Err(format!("Unknown argument: {}", arg).into());
            }
        }
    }

    Ok(RunOptions {
        input_dir,
        template_dir,
        match_threshold,
        render_dpi,
    })
}

fn extract_date(text: &str) -> Option<String> {
    for pattern in DATE_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(text) {
            if let Some(matched) = caps.get(1) {
                return Some(matched.as_str().to_string());
            }
        }
    }
    None
}

fn extract_amount(text: &str) -> Option<String> {
    for pattern in AMOUNT_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(text) {
            if let Some(matched) = caps.get(1) {
                return Some(matched.as_str().to_string());
            }
        }
    }
    None
}

fn extract_bill_type(text: &str) -> String {
    if GAS_RE.is_match(text) {
        "Gas".to_string()
    } else if ELECTRIC_RE.is_match(text) {
        "Electric".to_string()
    } else {
        "Unknown".to_string()
    }
}

fn extract_account_number(text: &str) -> Option<String> {
    for pattern in ACCOUNT_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(text) {
            if let Some(matched) = caps.get(1) {
                return Some(matched.as_str().to_string());
            }
        }
    }
    None
}

fn extract_tariff_and_billing_period(
    text: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let date_pattern_part = r"\d{1,2}(?:st|nd|rd|th)?\s+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{2,4}";
    let tariff_names = ["Cosy Octopus", "Agile Octopus", "Octopus Tracker"];

    for tariff_name in tariff_names.iter() {
        let pattern = format!(
            r"(?i)({})\s*\(\s*({})\s*-\s*({})\s*\)",
            regex::escape(tariff_name),
            date_pattern_part,
            date_pattern_part
        );
        if let Ok(re) = Regex::new(&pattern) {
            if let Some(caps) = re.captures(text) {
                let tariff = caps.get(1).map(|m| m.as_str().trim().to_string());
                let start_date = caps.get(2).map(|m| m.as_str().trim().to_string());
                let end_date = caps.get(3).map(|m| m.as_str().trim().to_string());
                return (tariff, start_date, end_date);
            }
        }
    }

    (None, None, None)
}

fn calculate_fingerprint(text: &str) -> String {
    let lower = text.to_lowercase();
    let normalized = WHITESPACE_RE.replace_all(&lower, "");
    format!("{:x}", md5::compute(normalized.as_bytes()))
}

fn extract_meter_number(text: &str) -> Option<String> {
    for pattern in METER_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(text) {
            if let Some(matched) = caps.get(1) {
                return Some(matched.as_str().to_string());
            }
        }
    }
    None
}

fn extract_address(text: &str) -> Option<String> {
    for pattern in ADDRESS_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(text) {
            if let Some(matched) = caps.get(1) {
                let cleaned = WHITESPACE_RE
                    .replace_all(matched.as_str().trim(), " ")
                    .to_string();
                return Some(cleaned);
            }
        }
    }
    None
}

fn image_extensions() -> [&'static str; 5] {
    [".jpg", ".jpeg", ".png", ".tiff", ".bmp"]
}

fn filename_contains_screenshot(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_lowercase().contains("screenshot"))
        .unwrap_or(false)
}

fn load_template_images(
    template_dir: &Path,
) -> Result<(Vec<TemplateImage>, HashSet<PathBuf>), Box<dyn Error>> {
    let extensions = image_extensions();
    let entries = fs::read_dir(template_dir)?;
    let mut all_paths = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && is_supported_image(&path, &extensions) {
            all_paths.push(path);
        }
    }

    let mut screenshot_paths: Vec<PathBuf> = all_paths
        .iter()
        .filter(|path| filename_contains_screenshot(path))
        .cloned()
        .collect();

    let use_paths = if screenshot_paths.is_empty() {
        all_paths
    } else {
        screenshot_paths.drain(..).collect()
    };

    let mut templates = Vec::new();
    let mut template_paths = HashSet::new();

    for path in use_paths {
        let image = image::open(&path)?;
        let gray = image.to_luma8();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("template")
            .to_string();

        templates.push(TemplateImage { name, image: gray });

        if let Ok(canonical) = fs::canonicalize(&path) {
            template_paths.insert(canonical);
        } else {
            template_paths.insert(path);
        }
    }

    Ok((templates, template_paths))
}

fn build_bill_record(source: &str, text: &str) -> BillRecord {
    let date = extract_date(text);
    let (tariff, start_date, end_date) = extract_tariff_and_billing_period(text);
    let amount = extract_amount(text);
    let bill_type = extract_bill_type(text);
    let account_number = extract_account_number(text);
    let meter_number = extract_meter_number(text);
    let address = extract_address(text);
    let fingerprint = calculate_fingerprint(text);

    BillRecord {
        filename: source.to_string(),
        date,
        tariff,
        start_date,
        end_date,
        amount,
        bill_type,
        account_number,
        meter_number,
        address,
        fingerprint,
    }
}

fn collect_pdf_files(folder_path: &Path) -> Vec<PathBuf> {
    let mut pdfs = Vec::new();
    let entries = match fs::read_dir(folder_path) {
        Ok(entries) => entries,
        Err(_) => return pdfs,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case("pdf") {
                    pdfs.push(path);
                }
            }
        }
    }

    pdfs.sort();
    pdfs
}

fn run_pdftoppm(pdf_path: &Path, dpi: u32, output_prefix: &Path) -> Result<(), Box<dyn Error>> {
    let pdf_path = pdf_path.to_str().ok_or("Invalid PDF path")?;
    let output_prefix = output_prefix.to_str().ok_or("Invalid output prefix")?;

    let status = Command::new("pdftoppm")
        .arg("-png")
        .arg("-r")
        .arg(dpi.to_string())
        .arg(pdf_path)
        .arg(output_prefix)
        .status();

    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("pdftoppm failed with status: {}", status).into()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err("pdftoppm not found. Install poppler (brew install poppler).".into())
        }
        Err(err) => Err(Box::new(err)),
    }
}

fn parse_page_number(file_name: &str, prefix: &str) -> Option<u32> {
    let suffix = file_name.strip_prefix(prefix)?;
    let suffix = suffix.trim_start_matches('-');
    let number_str = suffix.trim_end_matches(".png");
    number_str.parse::<u32>().ok()
}

fn collect_rendered_pages(temp_dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut pages = Vec::new();
    let entries = match fs::read_dir(temp_dir) {
        Ok(entries) => entries,
        Err(_) => return pages,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name.starts_with(prefix) && name.ends_with(".png") {
                pages.push(path);
            }
        }
    }

    pages.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| parse_page_number(name, prefix))
            .unwrap_or(0)
    });

    pages
}

/// Scales to try, in priority order. Bills from the same provider render at a
/// consistent size, so 1.0 almost always matches; the rest are fallbacks.
const MATCH_SCALES: [f32; 5] = [1.0, 0.85, 1.15, 0.75, 1.3];

/// A match this good at the current scale is accepted immediately, skipping
/// the remaining scales.
const CONFIDENT_MATCH_SCORE: f32 = 0.9;

fn prepare_templates(templates: &[TemplateImage]) -> Vec<PreparedTemplate> {
    templates
        .iter()
        .map(|template| {
            let scaled = MATCH_SCALES
                .iter()
                .filter_map(|scale| {
                    let width = (template.image.width() as f32 * scale).round() as u32;
                    let height = (template.image.height() as f32 * scale).round() as u32;
                    if width == 0 || height == 0 {
                        return None;
                    }
                    if (*scale - 1.0).abs() < f32::EPSILON {
                        Some(template.image.clone())
                    } else {
                        Some(resize(
                            &template.image,
                            width,
                            height,
                            image::imageops::FilterType::Lanczos3,
                        ))
                    }
                })
                .collect();
            PreparedTemplate {
                name: template.name.clone(),
                scaled,
            }
        })
        .collect()
}

/// Smallest 7-smooth number >= n, so rustfft gets an efficient transform size.
fn next_fast_fft_size(n: usize) -> usize {
    fn is_smooth(mut value: usize) -> bool {
        for p in [2usize, 3, 5, 7] {
            while value % p == 0 {
                value /= p;
            }
        }
        value == 1
    }
    let mut size = n.max(1);
    while !is_smooth(size) {
        size += 1;
    }
    size
}

fn fft2(
    data: &mut [Complex<f32>],
    width: usize,
    height: usize,
    inverse: bool,
    planner: &mut FftPlanner<f32>,
) {
    let direction = if inverse {
        FftDirection::Inverse
    } else {
        FftDirection::Forward
    };

    let row_fft = planner.plan_fft(width, direction);
    for row in data.chunks_exact_mut(width) {
        row_fft.process(row);
    }

    let col_fft = planner.plan_fft(height, direction);
    let mut column = vec![Complex::new(0.0, 0.0); height];
    for x in 0..width {
        for y in 0..height {
            column[y] = data[y * width + x];
        }
        col_fft.process(&mut column);
        for y in 0..height {
            data[y * width + x] = column[y];
        }
    }
}

/// Normalized cross-correlation via FFT: same score as imageproc's
/// `match_template` with `CrossCorrelationNormalized`
/// (sum(T*I) / sqrt(sum(T^2) * sum(I_patch^2))), but O(N log N) instead of
/// O(page_pixels * template_pixels). Returns (x, y, score) of the best match.
fn best_ncc_match(
    page: &GrayImage,
    template: &GrayImage,
    planner: &mut FftPlanner<f32>,
) -> Option<(u32, u32, f32)> {
    let (page_w, page_h) = page.dimensions();
    let (tpl_w, tpl_h) = template.dimensions();
    if tpl_w == 0 || tpl_h == 0 || tpl_w > page_w || tpl_h > page_h {
        return None;
    }

    let width = next_fast_fft_size((page_w + tpl_w - 1) as usize);
    let height = next_fast_fft_size((page_h + tpl_h - 1) as usize);
    let len = width * height;

    let zero = Complex::new(0.0, 0.0);
    let mut page_buf = vec![zero; len];
    for y in 0..page_h as usize {
        let offset = y * width;
        for x in 0..page_w as usize {
            page_buf[offset + x].re = page.get_pixel(x as u32, y as u32)[0] as f32;
        }
    }

    // Correlation via convolution with the template flipped on both axes:
    // N(x, y) = sum(T(u, v) * I(x+u, y+v)) = conv(I, T flipped)(x+tw-1, y+th-1)
    let mut tpl_buf = vec![zero; len];
    for v in 0..tpl_h as usize {
        let offset = v * width;
        for u in 0..tpl_w as usize {
            tpl_buf[offset + u].re =
                template.get_pixel(tpl_w - 1 - u as u32, tpl_h - 1 - v as u32)[0] as f32;
        }
    }

    fft2(&mut page_buf, width, height, false, planner);
    fft2(&mut tpl_buf, width, height, false, planner);
    for (p, t) in page_buf.iter_mut().zip(tpl_buf.iter()) {
        *p *= *t;
    }
    fft2(&mut page_buf, width, height, true, planner);
    let norm_factor = 1.0 / (len as f32);
    for p in page_buf.iter_mut() {
        *p *= norm_factor;
    }

    let squared_integral = integral_squared_image::<image::Luma<u8>, u64>(page);
    let template_ss: f64 = template.iter().map(|v| (*v as f64) * (*v as f64)).sum();

    let out_w = page_w - tpl_w + 1;
    let out_h = page_h - tpl_h + 1;
    let mut best_score = f64::NEG_INFINITY;
    let mut best_pos = (0u32, 0u32);

    for y in 0..out_h {
        let conv_row = ((y + tpl_h - 1) as usize) * width + (tpl_w - 1) as usize;
        for x in 0..out_w {
            let numerator = page_buf[conv_row + x as usize].re as f64;
            let patch_ss =
                sum_image_pixels(&squared_integral, x, y, x + tpl_w - 1, y + tpl_h - 1)[0] as f64;
            let norm = (template_ss * patch_ss).sqrt();
            let score = if norm > 0.0 {
                numerator / norm
            } else {
                numerator
            };
            if score > best_score {
                best_score = score;
                best_pos = (x, y);
            }
        }
    }

    Some((best_pos.0, best_pos.1, best_score as f32))
}

fn find_best_template_match(
    page: &GrayImage,
    template: &PreparedTemplate,
    threshold: f32,
    planner: &mut FftPlanner<f32>,
) -> Option<TemplateMatch> {
    let mut best: Option<TemplateMatch> = None;

    for resized in template.scaled.iter() {
        if resized.width() > page.width() || resized.height() > page.height() {
            continue;
        }

        if let Some((x, y, score)) = best_ncc_match(page, resized, planner) {
            if score >= threshold {
                let candidate = TemplateMatch {
                    template_name: template.name.clone(),
                    x,
                    y,
                    width: resized.width(),
                    height: resized.height(),
                    score,
                };

                if score >= CONFIDENT_MATCH_SCORE {
                    return Some(candidate);
                }

                match &best {
                    Some(best_match) if best_match.score >= score => {}
                    _ => best = Some(candidate),
                }
            }
        }
    }

    best
}

fn match_templates_on_page(
    page: &GrayImage,
    templates: &[PreparedTemplate],
    threshold: f32,
    planner: &mut FftPlanner<f32>,
) -> Vec<TemplateMatch> {
    let mut matches = Vec::new();

    for template in templates {
        if let Some(best) = find_best_template_match(page, template, threshold, planner) {
            matches.push(best);
        }
    }

    matches
}

fn ocr_matches_on_image(
    image_path: &Path,
    matches: &[TemplateMatch],
) -> Result<Vec<(TemplateMatch, String)>, Box<dyn Error>> {
    let mut ocr = LepTess::new(None, "eng")
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;

    let image_path = image_path
        .to_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Invalid image path"))?;

    if !ocr.set_image(image_path) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to set OCR image",
        )));
    }

    let mut results = Vec::new();
    for template_match in matches {
        let region = LepBox::new(
            template_match.x as i32,
            template_match.y as i32,
            template_match.width as i32,
            template_match.height as i32,
        );
        if let Some(region) = region {
            ocr.set_rectangle(&region);
            let text = ocr.get_utf8_text()?;
            results.push((template_match.clone(), text));
        }
    }

    Ok(results)
}

fn process_bill_pdfs(
    folder_path: &Path,
    templates: &[TemplateImage],
    match_threshold: f32,
    render_dpi: u32,
) -> Result<Vec<BillRecord>, Box<dyn Error>> {
    let pdfs = collect_pdf_files(folder_path);
    if pdfs.is_empty() || templates.is_empty() {
        return Ok(Vec::new());
    }

    let prepared_templates = prepare_templates(templates);
    let mut planner = FftPlanner::<f32>::new();
    let mut bill_data = Vec::new();

    for pdf_path in pdfs {
        let pdf_name = pdf_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("pdf");

        let temp_dir = TempDir::new()?;
        let output_prefix = temp_dir.path().join("page");
        run_pdftoppm(&pdf_path, render_dpi, &output_prefix)?;

        let prefix_name = output_prefix
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("page");

        let pages = collect_rendered_pages(temp_dir.path(), prefix_name);
        for (page_index, page_path) in pages.iter().enumerate() {
            let page_image = image::open(page_path)?;
            let gray = page_image.to_luma8();
            let matches =
                match_templates_on_page(&gray, &prepared_templates, match_threshold, &mut planner);
            if matches.is_empty() {
                continue;
            }

            let ocr_results = ocr_matches_on_image(page_path, &matches)?;
            for (template_match, text) in ocr_results {
                let source = format!(
                    "{}#page{}:{}@{},{}",
                    pdf_name,
                    page_index + 1,
                    template_match.template_name,
                    template_match.x,
                    template_match.y
                );
                bill_data.push(build_bill_record(&source, &text));
                println!("Processed: {}", source);
            }
        }
    }

    Ok(bill_data)
}

fn process_bill_images(folder_path: &Path, skip_paths: &HashSet<PathBuf>) -> Vec<BillRecord> {
    let image_extensions = image_extensions();
    let mut bill_data = Vec::new();

    let entries = match fs::read_dir(folder_path) {
        Ok(entries) => entries,
        Err(err) => {
            eprintln!("Error reading directory {}: {}", folder_path.display(), err);
            return bill_data;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let filename = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        if !is_supported_image(&path, &image_extensions) {
            continue;
        }

        let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if skip_paths.contains(&canonical) {
            continue;
        }

        match extract_bill_from_image(&path, &filename) {
            Ok(record) => {
                bill_data.push(record);
                println!("Processed: {}", filename);
            }
            Err(err) => {
                eprintln!("Error processing {}: {}", filename, err);
            }
        }
    }

    bill_data
}

fn is_supported_image(path: &Path, extensions: &[&str]) -> bool {
    let ext = match path.extension().and_then(|s| s.to_str()) {
        Some(ext) => ext.to_lowercase(),
        None => return false,
    };
    let ext = format!(".{}", ext);
    extensions.iter().any(|allowed| allowed == &ext)
}

fn extract_bill_from_image(path: &Path, filename: &str) -> Result<BillRecord, Box<dyn Error>> {
    let mut ocr = LepTess::new(None, "eng")
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;

    let image_path = path
        .to_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "Invalid image path"))?;

    if !ocr.set_image(image_path) {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Failed to set OCR image",
        )));
    }
    let text = ocr.get_utf8_text()?;
    Ok(build_bill_record(filename, &text))
}

fn parse_date_for_sort(date_str: &str) -> Option<NaiveDate> {
    let trimmed = date_str.trim();
    if trimmed.is_empty() {
        return None;
    }

    let cleaned = ORDINAL_RE.replace_all(trimmed, "$1");
    let normalized = WHITESPACE_RE.replace_all(cleaned.as_ref(), " ");
    let candidate = normalized.trim();

    let formats = [
        "%d/%m/%Y",
        "%m/%d/%Y",
        "%d-%m-%Y",
        "%m-%d-%Y",
        "%d/%m/%y",
        "%m/%d/%y",
        "%d-%m-%y",
        "%m-%d-%y",
        "%d %b %Y",
        "%d %B %Y",
        "%b %d, %Y",
        "%B %d, %Y",
        "%b %d %Y",
        "%B %d %Y",
    ];

    for format in formats.iter() {
        if let Ok(parsed) = NaiveDate::parse_from_str(candidate, format) {
            return Some(parsed);
        }
    }

    None
}

fn sort_by_start_date(records: &mut [BillRecord]) {
    records.sort_by(|a, b| {
        let a_date = a
            .start_date
            .as_deref()
            .and_then(|value| parse_date_for_sort(value));
        let b_date = b
            .start_date
            .as_deref()
            .and_then(|value| parse_date_for_sort(value));

        match (a_date, b_date) {
            (Some(a_date), Some(b_date)) => b_date.cmp(&a_date),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    });
}

fn sum_amounts(records: &[BillRecord]) -> Option<f64> {
    let mut total = 0.0;
    let mut found = false;
    for record in records {
        if let Some(amount) = record.amount.as_ref() {
            if let Ok(value) = amount.parse::<f64>() {
                total += value;
                found = true;
            }
        }
    }

    if found {
        Some(total)
    } else {
        None
    }
}

fn to_field(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| " ".to_string())
}

fn write_bill_csv(
    path: &Path,
    records: &[BillRecord],
    total_amount: Option<f64>,
) -> Result<(), Box<dyn Error>> {
    let mut writer = WriterBuilder::new().from_path(path)?;
    writer.write_record([
        "Filename",
        "Date",
        "Tariff",
        "Start Date",
        "End Date",
        "Amount",
        "Type",
        "Account Number",
        "Meter Number",
        "Address",
        "Fingerprint",
    ])?;

    for record in records {
        writer.write_record([
            record.filename.clone(),
            to_field(&record.date),
            to_field(&record.tariff),
            to_field(&record.start_date),
            to_field(&record.end_date),
            to_field(&record.amount),
            record.bill_type.clone(),
            to_field(&record.account_number),
            to_field(&record.meter_number),
            to_field(&record.address),
            record.fingerprint.clone(),
        ])?;
    }

    if let Some(total) = total_amount {
        let total_row = [
            "Total".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            format!("{:.2}", total),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
        ];
        let split_row = [
            "Company/Personal Split (50%)".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            format!("{:.2}", total / 2.0),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
            " ".to_string(),
        ];
        writer.write_record(total_row)?;
        writer.write_record(split_row)?;
    }

    writer.flush()?;
    Ok(())
}

fn identify_duplicates(bill_data: &[BillRecord]) -> Vec<DuplicateEntry> {
    let mut duplicates = Vec::new();

    let mut fingerprint_map: HashMap<String, Vec<String>> = HashMap::new();
    for bill in bill_data {
        fingerprint_map
            .entry(bill.fingerprint.clone())
            .or_default()
            .push(bill.filename.clone());
    }

    for (fingerprint, files) in fingerprint_map {
        if files.len() > 1 {
            duplicates.push(DuplicateEntry {
                files,
                match_type: "Exact content match".to_string(),
                fingerprint: Some(fingerprint),
                date: None,
                amount: None,
                bill_type: None,
            });
        }
    }

    let mut grouped: HashMap<(String, String, String), Vec<&BillRecord>> = HashMap::new();
    for bill in bill_data {
        if let (Some(date), Some(amount)) = (bill.date.as_ref(), bill.amount.as_ref()) {
            grouped
                .entry((date.clone(), amount.clone(), bill.bill_type.clone()))
                .or_default()
                .push(bill);
        }
    }

    for ((date, amount, bill_type), group) in grouped {
        if group.len() > 1 {
            let fingerprints: HashSet<&String> =
                group.iter().map(|bill| &bill.fingerprint).collect();
            if fingerprints.len() > 1 {
                let files = group.iter().map(|bill| bill.filename.clone()).collect();
                duplicates.push(DuplicateEntry {
                    files,
                    match_type: "Same date, amount and type".to_string(),
                    fingerprint: None,
                    date: Some(date),
                    amount: Some(amount),
                    bill_type: Some(bill_type),
                });
            }
        }
    }

    duplicates
}

fn write_duplicates_csv(path: &Path, duplicates: &[DuplicateEntry]) -> Result<(), Box<dyn Error>> {
    let mut writer = WriterBuilder::new().from_path(path)?;
    writer.write_record([
        "Files",
        "Match Type",
        "Fingerprint",
        "Date",
        "Amount",
        "Type",
    ])?;

    for dupe in duplicates {
        let files = dupe.files.join(", ");
        writer.write_record([
            files,
            dupe.match_type.clone(),
            dupe.fingerprint.clone().unwrap_or_else(|| " ".to_string()),
            dupe.date.clone().unwrap_or_else(|| " ".to_string()),
            dupe.amount.clone().unwrap_or_else(|| " ".to_string()),
            dupe.bill_type.clone().unwrap_or_else(|| " ".to_string()),
        ])?;
    }

    writer.flush()?;
    Ok(())
}

fn print_summary(bill_data: &[BillRecord]) {
    println!("\nExtracted Bill Information:");
    for (index, bill) in bill_data.iter().enumerate() {
        println!("\nBill {}: {}", index + 1, bill.filename);
        println!("  Type: {}", bill.bill_type);
        println!("  Date: {}", bill.date.as_deref().unwrap_or(" "));
        println!("  Tariff: {}", bill.tariff.as_deref().unwrap_or(" "));
        println!(
            "  Period Start: {}",
            bill.start_date.as_deref().unwrap_or(" ")
        );
        println!("  Period End: {}", bill.end_date.as_deref().unwrap_or(" "));

        let amount_display = match bill.amount.as_deref() {
            Some(value) if !value.trim().is_empty() => format!("\u{00A3}{}", value),
            _ => " ".to_string(),
        };
        println!("  Amount: {}", amount_display);

        println!(
            "  Account Number: {}",
            bill.account_number.as_deref().unwrap_or(" ")
        );
        println!(
            "  Meter Number: {}",
            bill.meter_number.as_deref().unwrap_or(" ")
        );
        println!("  Address: {}", bill.address.as_deref().unwrap_or(" "));
    }
}

fn main() -> Result<(), Box<dyn Error>> {    let current_dir = env::current_dir()?;
    let options = parse_args(&current_dir)?;
    println!("Starting bill analysis...");

    let (templates, template_paths) = load_template_images(&options.template_dir)?;
    if templates.is_empty() {
        println!("No template images found. PDF matching will be skipped.");
    }

    let mut bill_data = process_bill_images(&options.input_dir, &template_paths);
    let pdf_records = process_bill_pdfs(
        &options.input_dir,
        &templates,
        options.match_threshold,
        options.render_dpi,
    )?;
    bill_data.extend(pdf_records);

    if bill_data.is_empty() {
        println!("No bill images or PDF matches found.");
        return Ok(());
    }

    let total_amount = sum_amounts(&bill_data);

    let mut csv_records = bill_data.clone();
    sort_by_start_date(&mut csv_records);
    let csv_path = options.input_dir.join("bill_data.csv");
    write_bill_csv(&csv_path, &csv_records, total_amount)?;
    println!("Bill data saved to {}", csv_path.display());

    print_summary(&bill_data);

    let duplicates = identify_duplicates(&bill_data);
    if !duplicates.is_empty() {
        println!("\nPotential duplicate bills found:");
        for (index, dupe) in duplicates.iter().enumerate() {
            println!("\nDuplicate Set {}:", index + 1);
            println!("Match Type: {}", dupe.match_type);
            println!("Files: {}", dupe.files.join(", "));
            if let Some(date) = dupe.date.as_deref() {
                println!("Date: {}", date);
            }
            if let Some(amount) = dupe.amount.as_deref() {
                println!("Amount: {}", amount);
            }
            if let Some(bill_type) = dupe.bill_type.as_deref() {
                println!("Type: {}", bill_type);
            }
        }

        let dupes_csv_path = options.input_dir.join("duplicate_bills.csv");
        write_duplicates_csv(&dupes_csv_path, &duplicates)?;
        println!(
            "\nDuplicate information saved to {}",
            dupes_csv_path.display()
        );
    } else {
        println!("\nNo duplicate bills found.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use imageproc::template_matching::{find_extremes, match_template, MatchTemplateMethod};

    /// The FFT matcher must agree with imageproc's naive implementation
    /// (position and score) on the exact same metric.
    #[test]
    fn fft_ncc_matches_imageproc() {
        let (w, h, tw, th) = (97u32, 83u32, 21u32, 17u32);
        let mut page = GrayImage::new(w, h);
        let mut template = GrayImage::new(tw, th);

        // Deterministic pseudo-random fill (xorshift), no external crates.
        let mut state = 0x12345678u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % 251) as u8
        };
        for p in page.pixels_mut() {
            p[0] = next();
        }
        for p in template.pixels_mut() {
            p[0] = next();
        }
        // Plant the template at a known location so the peak is unambiguous.
        for v in 0..th {
            for u in 0..tw {
                let px = template.get_pixel(u, v)[0];
                page.put_pixel(40 + u, 30 + v, image::Luma([px]));
            }
        }

        let reference = match_template(&page, &template, MatchTemplateMethod::CrossCorrelationNormalized);
        let extremes = find_extremes(&reference);

        let mut planner = FftPlanner::<f32>::new();
        let (x, y, score) = best_ncc_match(&page, &template, &mut planner).unwrap();

        assert_eq!((x, y), extremes.max_value_location);
        assert!((score - extremes.max_value).abs() < 1e-3);
    }
}
