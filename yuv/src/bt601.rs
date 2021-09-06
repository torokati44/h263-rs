//! YUV-to-RGB decode

use lazy_static::lazy_static;

fn clamped_index(width: i32, height: i32, x: i32, y: i32) -> usize {
    (x.clamp(0, width - 1) + (y.clamp(0, height - 1) * width)) as usize
}

fn unclamped_index(width: i32, x: i32, y: i32) -> usize {
    (x + y * width) as usize
}

fn sample_chroma_for_luma(
    chroma: &[u8],
    chroma_width: usize,
    chroma_height: usize,
    luma_x: usize,
    luma_y: usize,
    clamp: bool,
) -> u8 {
    let width = chroma_width as i32;
    let height = chroma_height as i32;

    let sample_00;
    let sample_01;
    let sample_10;
    let sample_11;

    if clamp {
        let chroma_x = if luma_x == 0 {
            -1
        } else {
            (luma_x as i32 - 1) / 2
        };
        let chroma_y = if luma_y == 0 {
            -1
        } else {
            (luma_y as i32 - 1) / 2
        };

        debug_assert!(clamped_index(width, height, chroma_x + 1, chroma_y + 1) < chroma.len());
        unsafe {
            sample_00 =
                *chroma.get_unchecked(clamped_index(width, height, chroma_x, chroma_y)) as u16;
            sample_10 =
                *chroma.get_unchecked(clamped_index(width, height, chroma_x + 1, chroma_y)) as u16;
            sample_01 =
                *chroma.get_unchecked(clamped_index(width, height, chroma_x, chroma_y + 1)) as u16;
            sample_11 =
                *chroma.get_unchecked(clamped_index(width, height, chroma_x + 1, chroma_y + 1))
                    as u16;
        }
    } else {
        let chroma_x = (luma_x as i32 - 1) / 2;
        let chroma_y = (luma_y as i32 - 1) / 2;

        let base = unclamped_index(width, chroma_x, chroma_y);

        debug_assert!(base + chroma_width + 1 < chroma.len());
        unsafe {
            sample_00 = *chroma.get_unchecked(base) as u16;
            sample_10 = *chroma.get_unchecked(base + 1) as u16;
            sample_01 = *chroma.get_unchecked(base + chroma_width) as u16;
            sample_11 = *chroma.get_unchecked(base + chroma_width + 1) as u16;
        }
    }

    let interp_left = luma_x % 2 != 0;
    let interp_top = luma_y % 2 != 0;

    let mut sample: u16 = 0;
    sample += sample_00 * if interp_left { 3 } else { 1 };
    sample += sample_10 * if interp_left { 1 } else { 3 };

    sample += sample_01 * if interp_left { 3 } else { 1 };
    sample += sample_11 * if interp_left { 1 } else { 3 };

    sample += sample_00 * if interp_top { 3 } else { 1 };
    sample += sample_01 * if interp_top { 1 } else { 3 };

    sample += sample_10 * if interp_top { 3 } else { 1 };
    sample += sample_11 * if interp_top { 1 } else { 3 };

    ((sample + 8) / 16) as u8
}

/// Precomputes and stores the linear functions for converting YUV (YCb'Cr' to be precise)
/// colors to RGB (sRGB-like, with gamma) colors, in signed 12.4 fixed-point integer format.
///
/// Since the incoming components are u8, and there is only ever at most 3 of them added
/// at once (when computing the G channel), only about 10 bits would be used if they were
/// u8 - so to get some more precision (and reduce potential stepping artifacts), might
/// as well use about 14 of the 15 (not counting the sign bit) available in i16.
struct LUTs {
    /// the contribution of the Y component into all RGB channels
    pub y_to_gray: [i16; 256],
    /// the contribution of the V (Cr') component into the R channel
    pub cr_to_r: [i16; 256],
    /// the contribution of the V (Cr') component into the G channel
    pub cr_to_g: [i16; 256],
    /// the contribution of the U (Cb') component into the G channel
    pub cb_to_g: [i16; 256],
    /// the contribution of the U (Cb') component into the B channel
    pub cb_to_b: [i16; 256],
}

impl LUTs {
    pub fn new() -> LUTs {
        // - Y needs to be remapped linearly from 16..235 to 0..255
        // - Cr' and Cb' (a.k.a. V and U) need to be remapped linearly from 16..240 to 0..255,
        //     then shifted to -128..127, and then scaled by the appropriate coefficients
        // - Finally all values are multiplied by 16 (1<<4) to turn them into 12.4 format, and rounded to integer.
        fn remap_luma(luma: f32) -> i16 {
            ((luma - 16.0) * (255.0 / (235.0 - 16.0)) * 16.0).round() as i16
        }
        fn remap_chroma(chroma: f32, coeff: f32) -> i16 {
            (((chroma - 16.0) * (255.0 / (240.0 - 16.0)) - 128.0) * coeff * 16.0).round() as i16
        }

        let mut y_to_gray = [0i16; 256];
        let mut cr_to_r = [0i16; 256];
        let mut cr_to_g = [0i16; 256];
        let mut cb_to_g = [0i16; 256];
        let mut cb_to_b = [0i16; 256];

        for i in 0..256 {
            let f = i as f32;
            y_to_gray[i] = remap_luma(f);
            cr_to_r[i] = remap_chroma(f, 1.370705); // sanity check: Cr' contributes "positively" to R
            cr_to_g[i] = remap_chroma(f, -0.698001); // sanity check: Cr' contributes "negatively" to G
            cb_to_g[i] = remap_chroma(f, -0.337633); // sanity check: Cb' contributes "negatively" to G
            cb_to_b[i] = remap_chroma(f, 1.732446); // sanity check: Cb' contributes "positively" to B
        }

        LUTs {
            y_to_gray,
            cr_to_r,
            cr_to_g,
            cb_to_g,
            cb_to_b,
        }
    }
}

lazy_static! {
    static ref LUTS: LUTs = LUTs::new();
}

#[inline]
fn yuv_to_rgb(yuv: (u8, u8,u8), luts: &LUTs) -> (u8, u8, u8) {
    let (y, cb, cr) = yuv;

    // We rely on the optimizers in rustc/LLVM to eliminate the bounds checks when indexing
    // into the fixed 256-long arrays in `luts` with indices coming in as `u8` parameters.
    // This is crucial for performance, as this function runs in a fairly tight loop, on all pixels.
    // I verified that this is actually happening, see here: https://rust.godbolt.org/z/vWzesYzbq
    // And benchmarking showed no time difference from an `unsafe` + `get_unchecked()` solution.

    let y = luts.y_to_gray[y as usize];

    // The `(... + 8) >> 4` parts convert back from 12.4 fixed-point to `u8` with correct rounding.
    // (At least for positive numbers - any negative numbers that might occur will be clamped to 0 anyway.)
    let r = (y + luts.cr_to_r[cr as usize] + 8) >> 4;
    let g = (y + luts.cr_to_g[cr as usize] + luts.cb_to_g[cb as usize] + 8) >> 4;
    let b = (y + luts.cb_to_b[cb as usize] + 8) >> 4;

    (r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8)
}


#[inline]
fn interp_chroma_quarter_toward(a: &(u8, u8, u8), b: &(u8, u8, u8)) -> (u8, u8, u8) {
    let cb = a.1 as u16;
    let cr = a.2 as u16;

    let new_cb = (cb + cb + cb + b.1 as u16 + 2) / 4;
    let new_cr = (cr + cr + cr + b.2 as u16 + 2) / 4;

    (a.0, new_cb as u8, new_cr as u8)
}


/// Convert YUV 4:2:0 data into RGB 1:1:1 data.
///
/// This function yields an RGBA picture with the same number of pixels as were
/// provided in the `y` picture. The `b` and `r` pictures will be resampled at
/// this stage, and the resulting picture will have color components mixed.
pub fn yuv420_to_rgba(
    y: &[u8],
    chroma_b: &[u8],
    chroma_r: &[u8],
    y_width: usize,
    br_width: usize,
) -> Vec<u8> {
    let y_height = y.len() / y_width;
    let br_height = chroma_b.len() / br_width;

    // prefilling with 255, so the tight loop won't need to write to the alpha channel
    let mut rgba = vec![255; y.len() * 4];
    let rgba_stride = y_width * 4;

    // making sure that the "is it initialized already?" check is only done once per frame by getting a direct reference
    let luts: &LUTs = &*LUTS;


    fn get_two_rows(of: &[u8], from: usize, width: usize) -> (&[u8], &[u8]) {
        let (_before, rows): (&[u8], &[u8]) = of.split_at(from);
        let (top_row, rest): (&[u8], &[u8]) = rows.split_at(width);
        let (bottom_row, _rest): (&[u8], &[u8]) = rest.split_at(width);

        (top_row, bottom_row)
    }


    fn get_two_rows_mut(of: &mut [u8], from: usize, width: usize) -> (&mut [u8], &mut [u8]) {
        let (_before, rows): (&mut [u8], &mut [u8]) = of.split_at_mut(from);
        let (top_row, rest): (&mut [u8], &mut [u8]) = rows.split_at_mut(width);
        let (bottom_row, _rest): (&mut [u8], &mut [u8]) = rest.split_at_mut(width);

        (top_row, bottom_row)
    }




    for chroma_row in 0..br_height-1 {
        let luma_row = chroma_row * 2;

        let (y_upper, y_lower) = get_two_rows(&y, luma_row*y_width, y_width);
        let (cb_upper, cb_lower) = get_two_rows(&chroma_b, chroma_row*br_width, br_width);
        let (cr_upper, cr_lower) = get_two_rows(&chroma_r, chroma_row*br_width, br_width);
        let (rgba_upper, rgba_lower) = get_two_rows_mut(&mut rgba, luma_row*rgba_stride, rgba_stride);

        let y_iter = y_upper.chunks(2).zip(y_lower.chunks(2));
        let cb_iter = cb_upper.windows(2).zip(cb_lower.windows(2));
        let cr_iter = cr_upper.windows(2).zip(cr_lower.windows(2));
        let rgba_iter = rgba_upper.chunks_mut(8).zip(rgba_lower.chunks_mut(8));



        for ((((cb_u, cb_l), (cr_u, cr_l)), (y_u, y_l)), (rgba_u, rgba_l)) in cb_iter.zip(cr_iter).zip(y_iter).zip(rgba_iter) {

            // TODO move to one level up, and assign right* to these after each quad-iter
            let lefttop = (y_u[0], cb_u[0], cr_u[0]);
            let leftbot = (y_l[0], cb_l[0], cr_l[0]);

            let righttop = (y_u[1], cb_u[1], cr_u[1]);
            let rightbot = (y_l[1], cb_l[1], cr_l[1]);

            let top_l = interp_chroma_quarter_toward(&lefttop, &righttop);
            let top_r = interp_chroma_quarter_toward(&righttop,&lefttop);

            let bot_l = interp_chroma_quarter_toward(&leftbot, &rightbot);
            let bot_r = interp_chroma_quarter_toward(&rightbot, &leftbot);


            let tl = interp_chroma_quarter_toward(&top_l, &bot_l);
            let tr = interp_chroma_quarter_toward(&top_r, &bot_r);

            let bl = interp_chroma_quarter_toward(&bot_l, &top_l);
            let br = interp_chroma_quarter_toward(&bot_r, &top_r);


            let tl = yuv_to_rgb(tl.into(), &luts);
            let tr = yuv_to_rgb(tr.into(), &luts);

            let bl = yuv_to_rgb(bl.into(), &luts);
            let br = yuv_to_rgb(br.into(), &luts);


            rgba_u[0] = tl.0;
            rgba_u[1] = tl.1;
            rgba_u[2] = tl.2;

            rgba_u[4] = tr.0;
            rgba_u[5] = tr.1;
            rgba_u[6] = tr.2;

            rgba_l[0] = bl.0;
            rgba_l[1] = bl.1;
            rgba_l[2] = bl.2;

            rgba_l[4] = br.0;
            rgba_l[5] = br.1;
            rgba_l[6] = br.2;

        }



    }

/*
    // doing the sides with clamping
    for y_pos in 0..y_height {
        for x_pos in [0, y_width - 1].iter() {
            let y_sample = y.get(x_pos + y_pos * y_width).copied().unwrap_or(0);
            let b_sample =
                sample_chroma_for_luma(chroma_b, br_width, br_height, *x_pos, y_pos, true);
            let r_sample =
                sample_chroma_for_luma(chroma_r, br_width, br_height, *x_pos, y_pos, true);

            // just recomputing for every pixel, as there aren't any long continuous runs here
            base = (x_pos + y_pos * y_width) * 4;

            convert_and_write_pixel((y_sample, b_sample, r_sample), &mut rgba, base, luts);
        }
    }

    // doing the top and bottom edges with clamping
    for y_pos in [0, y_height - 1].iter() {
        base = y_pos * y_width * 4; // resetting to the leftmost pixel of the rows
        for x_pos in 0..y_width {
            let y_sample = y.get(x_pos + y_pos * y_width).copied().unwrap_or(0);
            let b_sample =
                sample_chroma_for_luma(chroma_b, br_width, br_height, x_pos, *y_pos, true);
            let r_sample =
                sample_chroma_for_luma(chroma_r, br_width, br_height, x_pos, *y_pos, true);

            convert_and_write_pixel((y_sample, b_sample, r_sample), &mut rgba, base, luts);
            base += 4; // advancing by one RGBA pixel
        }
    }
*/
    rgba
}
