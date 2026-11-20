//! `icon <in.svg> <out.png> <size>`: renders an SVG to a square PNG with resvg.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let [_, input, output, size] = a.as_slice() else {
        eprintln!("usage: icon <in.svg> <out.png> <size>");
        std::process::exit(2);
    };
    let size: u32 = size.parse().expect("size");
    let data = std::fs::read(input).expect("read svg");
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).expect("parse svg");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
    let s = tree.size();
    let scale = size as f32 / s.width().max(s.height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.save_png(output).expect("write png");
}
