//! Social name normalisation, sorting, crown prefixes and base-37 encoding.

/// Strips leading/trailing control characters and spaces (chars `<= ' '`).
pub fn trim_control_chars(text: &str) -> String {
    text.trim_matches(|c: char| c <= ' ').to_string()
}

/// The social text rows used by the friend/ignore owners, each a localised
/// message in the launch language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocialText {
    FriendListFullMembers,
    FriendListFull,
    FriendLogin,
    FriendLogout,
    FriendListDupe,
    IgnoreListFullMembers,
    IgnoreListFull,
    IgnoreListDupe,
    FriendCantAddSelf,
    IgnoreCantAddSelf,
    RemoveIgnore1,
    RemoveIgnore2,
    RemoveFriend1,
    RemoveFriend2,
}

impl SocialText {
    /// The message in the launch language.
    pub fn text(self) -> &'static str {
        use rs910_core::texts::Msg;
        match self {
            Self::FriendListFullMembers => Msg::FriendlistFullMembers,
            Self::FriendListFull => Msg::FriendlistFull,
            Self::FriendLogin => Msg::FriendLogin,
            Self::FriendLogout => Msg::FriendLogout,
            Self::FriendListDupe => Msg::FriendListDupe,
            Self::IgnoreListFullMembers => Msg::IgnoreListFullMembers,
            Self::IgnoreListFull => Msg::IgnoreListFull,
            Self::IgnoreListDupe => Msg::IgnoreListDupe,
            Self::FriendCantAddSelf => Msg::FriendCantAddSelf,
            Self::IgnoreCantAddSelf => Msg::IgnoreCantAddSelf,
            Self::RemoveIgnore1 => Msg::RemoveIgnore1,
            Self::RemoveIgnore2 => Msg::RemoveIgnore2,
            Self::RemoveFriend1 => Msg::RemoveFriend1,
            Self::RemoveFriend2 => Msg::RemoveFriend2,
        }
        .get()
    }
}

/// Characters trimmed from both ends of a name before normalization.
pub(super) fn namespace_trim_char(c: u16) -> bool {
    c == 160 || c == u16::from(b' ') || c == u16::from(b'_') || c == u16::from(b'-')
}

/// Characters that survive name normalization.
pub(super) fn namespace_valid_char(c: u16) -> bool {
    const VALID_1: &str = " \u{a0}_-àáâäãÀÁÂÄÃèéêëÈÉÊËíîïÍÎÏòóôöõÒÓÔÖÕùúûüÙÚÛÜçÇÿŸñÑß";
    let control = c < 0x20 || (0x7f..=0x9f).contains(&c);
    if control {
        return false;
    }
    if (u16::from(b'0')..=u16::from(b'9')).contains(&c)
        || (u16::from(b'A')..=u16::from(b'Z')).contains(&c)
        || (u16::from(b'a')..=u16::from(b'z')).contains(&c)
    {
        return true;
    }
    VALID_1.encode_utf16().any(|v| v == c) || "[]#".encode_utf16().any(|v| v == c)
}

/// Maps one name character to its normalized form (accents folded, separators to `_`).
pub(super) fn namespace_normalize_char(c: u16) -> u16 {
    let Some(ch) = char::from_u32(u32::from(c)) else {
        return c;
    };
    let mapped = match ch {
        ' ' | '-' | '_' | '\u{a0}' => '_',
        '#' | '[' | ']' => ch,
        'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'à' | 'á' | 'â' | 'ã' | 'ä' => 'a',
        'Ç' | 'ç' => 'c',
        'È' | 'É' | 'Ê' | 'Ë' | 'è' | 'é' | 'ê' | 'ë' => 'e',
        'Í' | 'Î' | 'Ï' | 'í' | 'î' | 'ï' => 'i',
        'Ñ' | 'ñ' => 'n',
        'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'Ù' | 'Ú' | 'Û' | 'Ü' | 'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ß' => 'b',
        'ÿ' | 'Ÿ' => 'y',
        // Character.toLowerCase(char): a single BMP code unit.
        other => {
            let mut lower = other.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) if (l as u32) <= 0xffff => l,
                _ => other,
            }
        }
    };
    mapped as u32 as u16
}

/// Normalizes a whole display name for the RuneScape/legacy namespace, whose
/// name length limit is 12.
pub fn namespace_normalize(name: &str) -> Option<String> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut start = 0;
    let mut end = units.len();
    while start < end && namespace_trim_char(units[start]) {
        start += 1;
    }
    while end > start && namespace_trim_char(units[end - 1]) {
        end -= 1;
    }
    let length = end - start;
    if !(1..=12).contains(&length) {
        return None;
    }
    let out: Vec<u16> = units[start..end]
        .iter()
        .filter(|&&c| namespace_valid_char(c))
        .map(|&c| namespace_normalize_char(c))
        .filter(|&c| c != 0)
        .collect();
    (!out.is_empty()).then(|| String::from_utf16_lossy(&out))
}

/// Whether two names refer to the same friend: `#`-prefixed names compare
/// exactly, others by their normalized forms.
pub(super) fn friend_related(a: &str, a_norm: &str, b: &str, b_norm: Option<&str>) -> bool {
    if a.starts_with('#') || b.starts_with('#') {
        a == b
    } else {
        b_norm == Some(a_norm)
    }
}

/// Strips a leading moderator/crown image tag (`<img=0>` or `<img=1>`), which
/// is seven characters long.
pub(super) fn strip_crown_prefix(name: String) -> String {
    if name.starts_with("<img=0>") || name.starts_with("<img=1>") {
        name[7..].to_string()
    } else {
        name
    }
}

/// Parity-biased quicksort of names with their slot array. The ordering of
/// equal names depends on element parity, which the scripts observe.
pub fn sort_names_with_slots(names: &mut [Option<Vec<u16>>], slots: &mut [i32]) {
    fn sort(names: &mut [Option<Vec<u16>>], slots: &mut [i32], lo: isize, hi: isize) {
        if lo >= hi {
            return;
        }
        let mid = ((lo + hi) / 2) as usize;
        let (lo_u, hi_u) = (lo as usize, hi as usize);
        let mut store = lo_u;
        names.swap(mid, hi_u);
        slots.swap(mid, hi_u);
        let pivot = names[hi_u].clone();
        let pivot_slot = slots[hi_u];
        for i in lo_u..hi_u {
            let take = match (&pivot, &names[i]) {
                (None, _) => true,
                (Some(p), Some(v)) => {
                    let order = match v.cmp(p) {
                        std::cmp::Ordering::Less => -1,
                        std::cmp::Ordering::Equal => 0,
                        std::cmp::Ordering::Greater => 1,
                    };
                    order < (i as i32 & 1)
                }
                (Some(_), None) => false,
            };
            if take {
                names.swap(i, store);
                slots.swap(i, store);
                store += 1;
            }
        }
        names[hi_u] = names[store].take();
        names[store] = pivot;
        slots[hi_u] = slots[store];
        slots[store] = pivot_slot;
        sort(names, slots, lo, store as isize - 1);
        sort(names, slots, store as isize + 1, hi);
    }
    let n = names.len() as isize;
    sort(names, slots, 0, n - 1);
}

/// Packs a name into its base-37 form.
pub fn to_base37(text: &str) -> i64 {
    let mut value: i64 = 0;
    for unit in text.encode_utf16() {
        value = value.wrapping_mul(37);
        let c = unit as i64;
        if (65..=90).contains(&c) {
            value += c + 1 - 65;
        } else if (97..=122).contains(&c) {
            value += c + 1 - 97;
        } else if (48..=57).contains(&c) {
            value += c + 27 - 48;
        }
        if value >= 177_917_621_779_460_413 {
            break;
        }
    }
    while value % 37 == 0 && value != 0 {
        value /= 37;
    }
    value
}

/// Unpacks a base-37 name: `_` digits become U+00A0 and
/// capitalise the preceding letter; the first letter is capitalised.
pub fn from_base37(mut value: i64) -> Option<String> {
    const ALPHABET: &[u8; 37] = b"_abcdefghijklmnopqrstuvwxyz0123456789";
    if value <= 0 || value >= 6_582_952_005_840_035_281 || value % 37 == 0 {
        return None;
    }
    let mut out: Vec<char> = Vec::new();
    while value != 0 {
        let digit = (value % 37) as usize;
        value /= 37;
        let mut c = ALPHABET[digit] as char;
        if c == '_' {
            if let Some(last) = out.last_mut() {
                *last = last.to_ascii_uppercase();
            }
            c = '\u{a0}';
        }
        out.push(c);
    }
    out.reverse();
    if let Some(first) = out.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    Some(out.into_iter().collect())
}
