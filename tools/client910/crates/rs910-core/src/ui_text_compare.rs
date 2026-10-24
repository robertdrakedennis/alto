//! The client's string comparator and the language ids it depends on.
//! Operates on UTF-16 units, including NUL and isolated surrogates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Language {
    En = 0,
    De = 1,
    Fr = 2,
    Pt = 3,
    Nl = 4,
    Es = 5,
    EsMx = 6,
}
impl Language {
    pub fn from_id(id: i32) -> Option<Self> {
        Some(match id {
            0 => Self::En,
            1 => Self::De,
            2 => Self::Fr,
            3 => Self::Pt,
            4 => Self::Nl,
            5 => Self::Es,
            6 => Self::EsMx,
            _ => return None,
        })
    }
}
pub fn casing(c: u16) -> (u16, u16, bool) {
    if c < 128 {
        return match c {
            65..=90 => (c, c + 32, true),
            97..=122 => (c - 32, c, false),
            _ => (c, c, false),
        };
    }
    (
        crate::char_case::to_upper(c),
        crate::char_case::to_lower(c),
        crate::char_case::is_upper_or_title(c),
    )
}
pub fn normalize(c: u16, language: Option<Language>) -> u16 {
    match c {
        192..=198 => 65,
        199 => 67,
        200..=203 => 69,
        204..=207 => 73,
        209 if language != Some(Language::Es) => 78,
        210..=214 => 79,
        217..=220 => 85,
        221 => 89,
        223 => 115,
        224..=230 => 97,
        231 => 99,
        232..=235 => 101,
        236..=239 => 105,
        241 if language != Some(Language::Es) => 110,
        242..=246 => 111,
        249..=252 => 117,
        253 | 255 => 121,
        338 => 79,
        339 => 111,
        376 => 89,
        _ => c,
    }
}
pub fn expansion(c: u16) -> u16 {
    match c {
        198 | 338 => 69,
        230 | 339 => 101,
        223 => 115,
        _ => 0,
    }
}
pub fn sort_key(mut c: u16, language: Option<Language>) -> i32 {
    let (_, lower, upper) = casing(c);
    let mut key = (c as i32) << 4;
    if upper {
        c = lower;
        key = ((c as i32) << 4) + 1;
    }
    if c == 241 && language == Some(Language::Es) {
        key = 1762;
    }
    key
}
fn folded_difference(a: u16, b: u16, language: Option<Language>) -> Option<i32> {
    if a != b && casing(a).0 != casing(b).0 {
        let a = casing(a).1;
        let b = casing(b).1;
        if a != b {
            return Some(sort_key(a, language) - sort_key(b, language));
        }
    }
    None
}
pub fn compare(a: &[u16], b: &[u16], language: Option<Language>) -> i32 {
    let (na, nb) = (a.len() as i32, b.len() as i32);
    let (mut ia, mut ib) = (0i32, 0i32);
    let (mut ca, mut cb) = (0u16, 0u16);
    while ia - (ca as i32) < na || ib - (cb as i32) < nb {
        if ia - (ca as i32) >= na {
            return -1;
        }
        if ib - (cb as i32) >= nb {
            return 1;
        }
        let x = if ca == 0 {
            let c = a[ia as usize];
            ia += 1;
            c
        } else {
            ca
        };
        let y = if cb == 0 {
            let c = b[ib as usize];
            ib += 1;
            c
        } else {
            cb
        };
        ca = expansion(x);
        cb = expansion(y);
        if let Some(d) = folded_difference(normalize(x, language), normalize(y, language), language)
        {
            return d;
        }
    }
    let n = na.min(nb);
    for i in 0..n {
        let (ia, ib) = if language == Some(Language::Fr) {
            (na - 1 - i, nb - 1 - i)
        } else {
            (i, i)
        };
        if let Some(d) = folded_difference(a[ia as usize], b[ib as usize], language) {
            return d;
        }
    }
    if na != nb {
        return na - nb;
    }
    for i in 0..n as usize {
        if a[i] != b[i] {
            return sort_key(a[i], language) - sort_key(b[i], language);
        }
    }
    0
}
