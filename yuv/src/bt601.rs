//! YUV-to-RGB decode

use lazy_static::lazy_static;

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

    let gray = luts.y_to_gray[y as usize];

    // The `(... + 8) >> 4` parts convert back from 12.4 fixed-point to `u8` with correct rounding.
    // (At least for positive numbers - any negative numbers that might occur will be clamped to 0 anyway.)
    let r = (gray + luts.cr_to_r[cr as usize] + 8) >> 4;
    let g = (gray + luts.cr_to_g[cr as usize] + luts.cb_to_g[cb as usize] + 8) >> 4;
    let b = (gray + luts.cb_to_b[cb as usize] + 8) >> 4;

    (r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8)
}


/// Performs a linear interpolation with fixed t=0.25 between a and b,
/// but only their .1 and .2 components, with proper rounding.
/// a.0 is passed through as the .0 component of the result without
/// touching it, and b.0 is completely ignored.
///
/// The naming refers to its practical use on (Y, Cb', Cr') color tuples.
#[inline]
fn lerp_chroma(a: &(u8, u8, u8), b: &(u8, u8, u8)) -> (u8, u8, u8) {
    let cb = a.1 as u16;
    let cr = a.2 as u16;

    let new_cb = (cb + cb + cb + b.1 as u16 + 2) / 4;
    let new_cr = (cr + cr + cr + b.2 as u16 + 2) / 4;

    (a.0, new_cb as u8, new_cr as u8)
}


/// Similar to `lerp_chroma`, but the interpolated components of the result
/// (.1 and .2) are not rounded and divided by 4, to keep more precision.
/// So they are returned as `u16`, having 4 times the value they actually
/// should - or you can think of them as being in 14.2 fixed-point format.
#[inline]
fn bilerp_chroma_step1(a: &(u8, u8, u8), b: &(u8, u8, u8)) -> (u8, u16, u16) {
    let cb = a.1 as u16;
    let cr = a.2 as u16;

    let new_cb = cb + cb + cb + b.1 as u16;
    let new_cr = cr + cr + cr + b.2 as u16;

    (a.0, new_cb, new_cr)
}


/// Similar to `lerp_chroma`, but takes the parameters in the format as returned
/// by `bilerp_chroma_step1`. At the end, it performs the rounding and division on
/// the interpolated components, so converts them back to the regular `u8` format.
#[inline]
fn bilerp_chroma_step2(a: &(u8, u16, u16), b: &(u8, u16, u16)) -> (u8, u8, u8) {
    // The division by 4 has to be done twice at this point, hence the / 16,
    // and the + 8 is for correct rounding.
    let new_cb = (a.1 + a.1 + a.1 + b.1 + 8) / 16;
    let new_cr = (a.2 + a.2 + a.2 + b.2 + 8) / 16;

    (a.0, new_cb as u8, new_cr as u8)
}


/// Returns two subslices of `of` as a tuple. Both are `width` long.
/// The first one starts at the index `start`, and the second one at `start + stride`.
///
/// Preconditions:
///  - `start + width <= of.len()`.
///  - `start + stride + width <= of.len()`.
///  - `stride >= width`
#[inline]
fn get_two_rows(of: &[u8], start: usize, width: usize, stride: usize) -> (&[u8], &[u8]) {
    debug_assert!(start + width <= of.len());
    debug_assert!(start + stride + width <= of.len());
    debug_assert!(stride >= width);

    let (top_row, rest): (&[u8], &[u8]) = (&of[start..]).split_at(width);
    // `width` number of elements are already split off into `top_row`, so only the
    // difference has to be skipped here.
    // And for the end index, `(stride - width) + width` works out to just `stride`.
    let bottom_row: &[u8] = &rest[(stride - width)..stride];
    (top_row, bottom_row)
}


/// Similar to `get_two_rows`, but the slices going in and out are all `mut`.
#[inline]
fn get_two_rows_mut(of: &mut [u8], start: usize, width: usize, stride: usize) -> (&mut [u8], &mut [u8]) {
    debug_assert!(start + width <= of.len());
    debug_assert!(start + stride + width <= of.len());
    debug_assert!(stride >= width);

    let (top_row, rest): (&mut [u8], &mut [u8]) = (&mut of[start..]).split_at_mut(width);
    // `width` number of elements are already split off into `top_row`, so only the
    // difference has to be skipped here.
    // And for the end index, `(stride - width) + width` works out to just `stride`.
    let bottom_row: &mut [u8] = &mut rest[(stride - width)..stride];
    (top_row, bottom_row)
}


/// Convert planar YUV 4:2:0 data into interleaved RGBA 8888 data.
///
/// This function yields an RGBA picture with the same number of pixels as were
/// provided in the `y` picture. The `b` and `r` pictures will be resampled at
/// this stage, and the resulting picture will have color components mixed.
///
/// Preconditions:
///  - `y.len()` must be an integer multiple of `y_width`
///  - `chroma_b.len()` and `chroma_r.len()` must both be integer multiples of `br_width`
///  - `chroma_b` and `chroma_r` must be the same size
///  - If `y_width` is even, `br_width` must be `y_width / 2`, otherwise, `(y_width + 1) / 2`
///  - With `y_height` computed as `y.len() / y_width`, and `br_height` as `chroma_b.len() / br_width`:
///    If `y_height` is even, `br_height` must be `y_height / 2`, otherwise, `(y_height + 1) / 2`
///    (So, either there is an "outer" column/row of luma samples on the right/bottom (similar to how
///    there always is on the left/top) or they are cut off - independently of each other)
pub fn yuv420_to_rgba(
    y: &[u8],
    chroma_b: &[u8],
    chroma_r: &[u8],
    y_width: usize,
    br_width: usize,
) -> Vec<u8> {
    debug_assert_eq!(y.len() % y_width, 0);
    debug_assert_eq!(chroma_b.len() % br_width, 0);
    debug_assert_eq!(chroma_r.len() % br_width, 0);
    debug_assert_eq!(chroma_b.len(), chroma_r.len());

    if y.is_empty() {
        return vec![];
    }

    let y_height = y.len() / y_width;
    let br_height = chroma_b.len() / br_width;

    // the + 1 will be dropped after division for even sizes
    debug_assert_eq!((y_width + 1) / 2, br_width);
    debug_assert_eq!((y_height + 1) / 2, br_height);

    let mut rgba = vec![0; y.len() * 4];
    let rgba_stride = y_width * 4; // 4 bytes per pixel, interleaved

    // making sure that the "is it initialized already?" check is only done once per frame by getting a direct reference
    let luts: &LUTs = &*LUTS;

    // About the algorithm below:
    //
    // Consider Figure 2/H.263 in the ITU-T H.263 Recommendation.
    //
    // Every iteration below works with a 2x2 "bunch" of neighbouring chrominance samples,
    // and the 2x2 luminance samples "enclosed by" these chrominance samples; writing to
    // the 2x2 output pixels in the same location in the picture as the luminance samples.
    //
    // This means that the topmost row and the leftmost column of output pixels is not covered
    // by this loop. On pictures of even width, the rightmost column isn't covered either;
    // and similarly, on pictures of even height, the bottommost row is left out as well.
    //
    // Initially, the chrominance samples are "further out" of these 2x2 rectangles than they
    // should be, so they are bilinearly interpolated to the location of the luminance samples.

    // Iteration is done in a row-major order to fit the slice layouts.
    for chroma_row in 0..br_height-1 {
        // Selecting two consecutive rows from all 3 input and the output slices to work with.
        // The top row of Y and RGBA has to be skipped, as well as the first sample/pixel of
        // each row. The width of the Y and RGBA rows is derived from br_width to make the
        // parity of y_width irrelevant.
        let luma_row = chroma_row * 2 + 1;

        let (y_upper, y_lower) = get_two_rows(&y, luma_row*y_width+1, 2*(br_width-1), y_width);
        let (cb_upper, cb_lower) = get_two_rows(&chroma_b, chroma_row*br_width, br_width, br_width);
        let (cr_upper, cr_lower) = get_two_rows(&chroma_r, chroma_row*br_width, br_width, br_width);
        let (rgba_upper, rgba_lower) = get_two_rows_mut(&mut rgba, luma_row*rgba_stride+4, 2*(br_width-1)*4, rgba_stride);

        // The Cb and Cr data has to be iterated on with overlaps, while every sample or pixel
        // of Y and RGBA data only has to be touched in one iteration.
        let y_iter = y_upper.chunks(2).zip(y_lower.chunks(2));
        let cb_iter = cb_upper.windows(2).zip(cb_lower.windows(2));
        let cr_iter = cr_upper.windows(2).zip(cr_lower.windows(2));
        // Similar to how Y is iterated on, but with 4 channels per pixel
        let rgba_iter = rgba_upper.chunks_mut(8).zip(rgba_lower.chunks_mut(8));

        for ((((y_u, y_l), (cb_u, cb_l)), (cr_u, cr_l)), (rgba_u, rgba_l)) in y_iter.zip(cb_iter).zip(cr_iter).zip(rgba_iter) {
            let topleft = (y_u[0], cb_u[0], cr_u[0]);
            let bottomleft = (y_l[0], cb_l[0], cr_l[0]);

            let topright = (y_u[1], cb_u[1], cr_u[1]);
            let bottomright = (y_l[1], cb_l[1], cr_l[1]);

            // Bringing in the chroma components to where they should be horizontally
            let topleft_intermediate = bilerp_chroma_step1(&topleft, &topright);
            let topright_intermediate = bilerp_chroma_step1(&topright,&topleft);

            let bottomleft_intermediate = bilerp_chroma_step1(&bottomleft, &bottomright);
            let bottomright_intermediate = bilerp_chroma_step1(&bottomright, &bottomleft);

            // Then putting them in the right place vertically as well
            let topleft_final = bilerp_chroma_step2(&topleft_intermediate, &bottomleft_intermediate);
            let bottomleft_final = bilerp_chroma_step2(&bottomleft_intermediate, &topleft_intermediate);

            let topright_final = bilerp_chroma_step2(&topright_intermediate, &bottomright_intermediate);
            let bottomright_final = bilerp_chroma_step2(&bottomright_intermediate, &topright_intermediate);

            // Now the colorspace conversion can be done on the colocated components
            let topleft_rgb = yuv_to_rgb(topleft_final.into(), &luts);
            let topright_rgb = yuv_to_rgb(topright_final.into(), &luts);

            let bottomleft_rgb = yuv_to_rgb(bottomleft_final.into(), &luts);
            let bottomright_rgb = yuv_to_rgb(bottomright_final.into(), &luts);

            // Finally they are written into the output array
            rgba_u.copy_from_slice(&[topleft_rgb.0, topleft_rgb.1, topleft_rgb.2, 255, topright_rgb.0, topright_rgb.1, topright_rgb.2, 255]);
            rgba_l.copy_from_slice(&[bottomleft_rgb.0, bottomleft_rgb.1, bottomleft_rgb.2, 255, bottomright_rgb.0, bottomright_rgb.1, bottomright_rgb.2, 255]);

            // Note: The unmodified "right" chroma components (both top and bottom, both cb and cr) could
            // potentially be reused in the next iteration as "left" components, thus removing the need to
            // iterate on 2-long windows of these slices, but I think everything is clearer this way.
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
