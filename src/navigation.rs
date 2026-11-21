use std::cmp::Ordering;

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (aa, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < aa.len() && j < bb.len() {
        if aa[i].is_ascii_digit() && bb[j].is_ascii_digit() {
            let start_i = i;
            let start_j = j;
            while i < aa.len() && aa[i].is_ascii_digit() { i += 1; }
            while j < bb.len() && bb[j].is_ascii_digit() { j += 1; }
            let da = &a[start_i..i].trim_start_matches('0');
            let db = &b[start_j..j].trim_start_matches('0');
            let ordering = da.len().cmp(&db.len()).then_with(|| da.cmp(db))
                .then_with(|| (i - start_i).cmp(&(j - start_j)));
            if ordering != Ordering::Equal { return ordering; }
        } else {
            let ordering = aa[i].to_ascii_lowercase().cmp(&bb[j].to_ascii_lowercase());
            if ordering != Ordering::Equal { return ordering; }
            i += 1;
            j += 1;
        }
    }
    (aa.len() - i).cmp(&(bb.len() - j)).then_with(|| a.cmp(b))
}

pub fn neighbors(index: usize, len: usize, direction: i32) -> Vec<usize> {
    let first = if direction < 0 { -1 } else { 1 };
    let offsets = [first, -first, 2 * first, -2 * first, 3 * first];
    offsets.into_iter().take(if direction == 0 { 4 } else { 5 })
        .filter_map(|step| index.checked_add_signed(step))
        .filter(|&i| i < len)
        .collect()
}

pub fn fit_scale(image: (f32, f32), viewport: (f32, f32)) -> f32 {
    (viewport.0.max(1.0) / image.0.max(1.0))
        .min(viewport.1.max(1.0) / image.1.max(1.0)).min(1.0)
}
pub fn point_wheel_step(remainder: &mut f32, points: f32) -> i32 {
    const THRESHOLD: f32 = 48.0;
    if points.signum() != remainder.signum() {
        *remainder = 0.0;
    }
    *remainder += points.clamp(-THRESHOLD * 30.0, THRESHOLD * 30.0);
    let ticks = (*remainder / THRESHOLD) as i32;
    *remainder -= ticks as f32 * THRESHOLD;
    -ticks
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_names_sort_naturally_and_deterministically() {
        let mut names = ["IMG_10.jpg", "img_02.jpg", "img_2.jpg", "IMG_1.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["IMG_1.jpg", "img_2.jpg", "img_02.jpg", "IMG_10.jpg"]);
    }
    #[test]
    fn prefetch_favors_direction_without_crossing_folder_edges() {
        assert_eq!(neighbors(5, 15, 1), vec![6, 4, 7, 3, 8]);
        assert_eq!(neighbors(5, 15, -1), vec![4, 6, 3, 7, 2]);
        assert_eq!(neighbors(0, 2, -1), vec![1]);
    }
    #[test]
    fn trackpad_accumulates_small_steps_and_reverses_without_stale_motion() {
        let mut remainder = 0.0;
        assert_eq!(point_wheel_step(&mut remainder, -12.0), 0);
        assert_eq!(point_wheel_step(&mut remainder, -44.0), 1);
        assert_eq!(point_wheel_step(&mut remainder, 98.0), -2);
        assert_eq!(remainder, 2.0);
    }
    #[test]
    fn fit_handles_landscape_and_portrait_without_upscaling() {
        assert_eq!(fit_scale((200.0, 100.0), (100.0, 100.0)), 0.5);
        assert_eq!(fit_scale((100.0, 200.0), (100.0, 100.0)), 0.5);
        assert_eq!(fit_scale((20.0, 10.0), (100.0, 100.0)), 1.0);
    }
}
