//! The quicksort the stock tool sorts its lists with. Its order for equal elements is part of what the tool writes (which of
//! several triangles on one edge pairs first, which of several equally heavy triangles is looked at first), so the same
//! procedure is needed to reproduce it.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

/// Sort `list` by `less`. The pivot is the element in the middle of the range, an element moves when it is on the wrong
/// side of the pivot strictly, and elements that are neither stay where the partition leaves them. The sort is not
/// stable: equal elements come out in the order this procedure gives them.
pub fn game_sort_by<T: Copy>(list: &mut [T], less: &dyn Fn(&T, &T) -> bool) {
    let n = list.len();
    if n > 1 {
        sort_range(list, 0, isize::try_from(n.saturating_sub(1)).unwrap_or(0), less);
    }
}

fn at<T>(list: &[T], i: isize) -> Option<&T> {
    usize::try_from(i).ok().and_then(|i| list.get(i))
}

fn sort_range<T: Copy>(list: &mut [T], lo: isize, hi: isize, less: &dyn Fn(&T, &T) -> bool) {
    let mut lo = lo;
    loop {
        let (mut i, mut j) = (lo, hi);
        let Some(&pivot) = at(list, lo.saturating_add(hi) >> 1) else {
            return;
        };
        loop {
            while at(list, i).is_some_and(|x| less(x, &pivot)) {
                i = i.saturating_add(1);
            }
            while at(list, j).is_some_and(|x| less(&pivot, x)) {
                j = j.saturating_sub(1);
            }
            if j < i {
                break;
            }
            if j != i
                && let (Ok(a), Ok(b)) = (usize::try_from(i), usize::try_from(j))
            {
                list.swap(a, b);
            }
            j = j.saturating_sub(1);
            i = i.saturating_add(1);
            if i > j {
                break;
            }
        }
        if lo < j {
            sort_range(list, lo, j, less);
        }
        if hi <= i {
            return;
        }
        lo = i;
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "a test reads what it just built"
)]
mod tests {
    use super::game_sort_by;

    #[test]
    fn sorts_by_the_comparison_and_keeps_lengths() {
        let mut list: Vec<(u32, u32)> = (0..200u32).map(|i| (i, (i * 7919) % 31)).collect();
        game_sort_by(&mut list, &|a, b| a.1 < b.1);
        assert!(list.windows(2).all(|w| w[0].1 <= w[1].1));
        assert_eq!(list.len(), 200);
        let mut ids: Vec<u32> = list.iter().map(|p| p.0).collect();
        ids.sort_unstable();
        assert!(ids.iter().copied().eq(0..200));
    }

    #[test]
    fn short_lists_are_left_alone() {
        let mut none: [u8; 0] = [];
        game_sort_by(&mut none, &|a, b| a < b);
        let mut one = [5u8];
        game_sort_by(&mut one, &|a, b| a < b);
        assert_eq!(one, [5]);
    }
}
