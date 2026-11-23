//! 拷貝主體: a picture's subject without its background, as the Mac's Photos and Preview
//! copy it, found by IS-Net, a segmentation model, run by the system's ONNX Runtime.

use std::path::PathBuf;

use gtk::glib;

use super::imaging::Rgba;

/// The model, downloaded once into `model_path`, and how much it is.
pub const MODEL_URL: &str = "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx";
pub const MODEL_SIZE: &str = "179 MB";
/// The square the model sees the picture in.
const SIDE: usize = 1024;
/// How sure the model must be of a pixel for the subject's bounds to take it in.
const SURE: f32 = 0.5;

pub fn model_path() -> PathBuf {
    glib::user_data_dir().join("nekoland/models/isnet-general-use.onnx")
}

/// The system's ONNX Runtime, which distributions name with its version alone.
// shortcut: looks in the usual library folders only; ORT_DYLIB_PATH names any other.
fn runtime() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        return Some(path.into());
    }
    ["/usr/lib/x86_64-linux-gnu", "/usr/lib/aarch64-linux-gnu", "/usr/lib64", "/usr/lib", "/usr/local/lib"]
        .iter()
        .filter_map(|folder| std::fs::read_dir(folder).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("libonnxruntime.so")))
}

/// The subject of `picture`, the rest made clear and cut away; none when the model finds
/// nothing.
pub fn subject(picture: &Rgba) -> Result<Option<Rgba>, String> {
    let library = runtime().ok_or("找不到 ONNX Runtime。")?;
    let failed = |err: ort::Error| err.to_string();
    ort::init_from(library.display().to_string()).commit().map_err(failed)?;
    let mut session = ort::session::Session::builder()
        .and_then(|builder| builder.commit_from_file(model_path()))
        .map_err(failed)?;
    let input = model_input(picture);
    let tensor = ort::value::TensorRef::from_array_view(([1usize, 3, SIDE, SIDE], &input[..])).map_err(failed)?;
    let outputs = session.run(ort::inputs![tensor]).map_err(failed)?;
    let (_, sure) = outputs[0].try_extract_tensor::<f32>().map_err(failed)?;
    if sure.len() < SIDE * SIDE {
        return Err("模型的結果不完整。".into());
    }
    Ok(cut_out(picture, &sure[..SIDE * SIDE]))
}

/// The picture squeezed into the model's square, channel after channel, as IS-Net was
/// trained on it: scaled by its brightest value, less a half.
fn model_input(picture: &Rgba) -> Vec<f32> {
    let mut input = vec![0.0; 3 * SIDE * SIDE];
    let (scale_x, scale_y) = (picture.width as f32 / SIDE as f32, picture.height as f32 / SIDE as f32);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (from_x, from_y) = ((x as f32 + 0.5) * scale_x - 0.5, (y as f32 + 0.5) * scale_y - 0.5);
            for channel in 0..3 {
                input[channel * SIDE * SIDE + y * SIDE + x] =
                    bilinear(picture.width, picture.height, from_x, from_y, |x, y| f32::from(picture.pixel(x, y)[channel]));
            }
        }
    }
    let brightest = input.iter().copied().fold(1e-6, f32::max);
    input.iter_mut().for_each(|value| *value = *value / brightest - 0.5);
    input
}

/// The picture with what the model is sure of, as alpha, cut to the bounds of what it is sure
/// of more than not.
fn cut_out(picture: &Rgba, sure: &[f32]) -> Option<Rgba> {
    let (least, most) = sure.iter().fold((f32::MAX, f32::MIN), |(least, most), value| (least.min(*value), most.max(*value)));
    if most - least < f32::EPSILON {
        return None;
    }
    let (width, height) = (picture.width, picture.height);
    let (scale_x, scale_y) = (SIDE as f32 / width as f32, SIDE as f32 / height as f32);
    let alpha: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let (at_x, at_y) = ((x as f32 + 0.5) * scale_x - 0.5, (y as f32 + 0.5) * scale_y - 0.5);
            (bilinear(SIDE, SIDE, at_x, at_y, |x, y| sure[y * SIDE + x]) - least) / (most - least)
        })
        .collect();
    let taken = |x: usize, y: usize| alpha[y * width + x] >= SURE;
    let rows: Vec<usize> = (0..height).filter(|&y| (0..width).any(|x| taken(x, y))).collect();
    let columns: Vec<usize> = (0..width).filter(|&x| (0..height).any(|y| taken(x, y))).collect();
    let (&top, &bottom, &left, &right) = (rows.first()?, rows.last()?, columns.first()?, columns.last()?);
    let mut bytes = Vec::with_capacity((right - left + 1) * (bottom - top + 1) * 4);
    for y in top..=bottom {
        for x in left..=right {
            let mut pixel = picture.pixel(x, y);
            pixel[3] = (f32::from(pixel[3]) * alpha[y * width + x]).round() as u8;
            bytes.extend_from_slice(&pixel);
        }
    }
    Some(Rgba { width: right - left + 1, height: bottom - top + 1, bytes })
}

/// `value` between the four pixels around (`x`, `y`), those off the edge taken from it.
fn bilinear(width: usize, height: usize, x: f32, y: f32, value: impl Fn(usize, usize) -> f32) -> f32 {
    let (x, y) = (x.clamp(0.0, (width - 1) as f32), y.clamp(0.0, (height - 1) as f32));
    let (left, top) = (x.floor() as usize, y.floor() as usize);
    let (right, bottom) = ((left + 1).min(width - 1), (top + 1).min(height - 1));
    let (across, down) = (x - left as f32, y - top as f32);
    let upper = value(left, top) * (1.0 - across) + value(right, top) * across;
    let lower = value(left, bottom) * (1.0 - across) + value(right, bottom) * across;
    upper * (1.0 - down) + lower * down
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_subject_is_cut_to_what_the_model_is_sure_of() {
        let picture = Rgba { width: 4, height: 4, bytes: vec![200; 64] };
        // Sure of the square's middle quarter only.
        let sure: Vec<f32> = (0..SIDE * SIDE)
            .map(|at| {
                let (x, y) = (at % SIDE, at / SIDE);
                if (SIDE / 4..3 * SIDE / 4).contains(&x) && (SIDE / 4..3 * SIDE / 4).contains(&y) { 1.0 } else { 0.0 }
            })
            .collect();
        let subject = cut_out(&picture, &sure).expect("a subject");
        assert_eq!((subject.width, subject.height), (2, 2));
        assert!(subject.bytes.chunks(4).all(|pixel| pixel[3] > 128));
        assert!(cut_out(&picture, &vec![0.3; SIDE * SIDE]).is_none());
    }
}
