//! ISA extensions and ISA strings

#[cfg(test)]
mod tests;

use core::cmp::Ordering;

/// Maximum number of ISA extensions supported in a single instruction set
pub const MAX_ISA_EXTENSIONS: usize = 128;
/// Maximum length of an ISA string in bytes
pub const MAX_ISA_STRING_LENGTH: usize = 2048;

/// Canonical order of single-letter extensions, which is also used to order multi-letter `Z`
/// extensions by their second letter, see "ISA Extension Naming Conventions" chapter of the RISC-V
/// unprivileged specification
const CANONICAL_EXTENSION_ORDER: &[u8] = b"iemafdqlcbkjtpvh";

/// ISA extension with its version
#[derive(Debug, Clone, Copy)]
#[derive_const(PartialEq, Eq)]
pub struct IsaExtension {
    /// Name of the extension as used in ISA strings, for example, `zcmp`
    pub name: &'static str,
    /// Major version of the extension
    pub major_version: u8,
    /// Minor version of the extension
    pub minor_version: u8,
}

impl IsaExtension {
    const PLACEHOLDER: Self = Self::new("", 0, 0);

    /// Create a new instance
    #[inline(always)]
    pub const fn new(name: &'static str, major_version: u8, minor_version: u8) -> Self {
        Self {
            name,
            major_version,
            minor_version,
        }
    }

    /// Combine multiple lists of extensions into a single list in canonical order without
    /// duplicates.
    ///
    /// The first `len` elements of the returned array (`(array, len)`) contain the result. This is
    /// meant to be used at compile time, and panics if there are more than [`MAX_ISA_EXTENSIONS`]
    /// unique extensions.
    pub const fn canonical_set(extension_lists: &[&[Self]]) -> ([Self; MAX_ISA_EXTENSIONS], usize) {
        let mut extensions = [Self::PLACEHOLDER; MAX_ISA_EXTENSIONS];
        let mut len = 0;

        // For loops are not yet usable in const environment
        let mut list_index = 0;
        while list_index < extension_lists.len() {
            let extension_list = extension_lists[list_index];
            list_index += 1;

            let mut extension_index = 0;
            while extension_index < extension_list.len() {
                let extension = extension_list[extension_index];
                extension_index += 1;

                // Find the position of the extension in already sorted extensions
                let mut position = 0;
                while position < len
                    && canonical_cmp(extensions[position].name, extension.name).is_lt()
                {
                    position += 1;
                }
                if position < len
                    && canonical_cmp(extensions[position].name, extension.name).is_eq()
                {
                    // Duplicate
                    continue;
                }

                assert!(len < MAX_ISA_EXTENSIONS, "Too many ISA extensions");

                // Shift the rest to insert the extension in the right place
                let mut index = len;
                while index > position {
                    extensions[index] = extensions[index - 1];
                    index -= 1;
                }
                extensions[position] = extension;
                len += 1;
            }
        }

        (extensions, len)
    }

    /// Format an ISA string as used in the `Tag_RISCV_arch` attribute of ELF files, for example,
    /// `rv64i2p1_m2p0_zmmul1p0`.
    ///
    /// Extensions are expected to be in canonical order (see [`Self::canonical_set()`]) with base
    /// ISA (`i` or `e`) first.
    ///
    /// The first `len` bytes of the returned array (`(array, len)`) contain the result. This is
    /// meant to be used at compile time, and panics if the string is longer than
    /// [`MAX_ISA_STRING_LENGTH`] bytes.
    pub const fn isa_string(xlen: u8, extensions: &[Self]) -> ([u8; MAX_ISA_STRING_LENGTH], usize) {
        let mut isa_string = [0; MAX_ISA_STRING_LENGTH];
        let mut len = 0;

        len = write_bytes(&mut isa_string, len, b"rv");
        len = write_number(&mut isa_string, len, xlen);

        let mut index = 0;
        while index < extensions.len() {
            let extension = extensions[index];
            if index > 0 {
                len = write_bytes(&mut isa_string, len, b"_");
            }
            index += 1;

            len = write_bytes(&mut isa_string, len, extension.name.as_bytes());
            len = write_number(&mut isa_string, len, extension.major_version);
            len = write_bytes(&mut isa_string, len, b"p");
            len = write_number(&mut isa_string, len, extension.minor_version);
        }

        (isa_string, len)
    }
}

const fn write_bytes(output: &mut [u8; MAX_ISA_STRING_LENGTH], len: usize, bytes: &[u8]) -> usize {
    assert!(
        len + bytes.len() <= MAX_ISA_STRING_LENGTH,
        "ISA string is too long"
    );

    let (_, target) = output.split_at_mut(len);
    let (target, _) = target.split_at_mut(bytes.len());
    target.copy_from_slice(bytes);

    len + bytes.len()
}

const fn write_number(output: &mut [u8; MAX_ISA_STRING_LENGTH], len: usize, number: u8) -> usize {
    let digits = [number / 100, number / 10 % 10, number % 10];
    let skip = if number >= 100 {
        0
    } else if number >= 10 {
        1
    } else {
        2
    };
    let (_, digits) = digits.split_at(skip);

    let mut len = len;
    let mut index = 0;
    while index < digits.len() {
        len = write_bytes(output, len, &[b'0' + digits[index]]);
        index += 1;
    }

    len
}

/// Position of a letter in canonical order, letters that are not in the list go after all others
const fn category(letter: u8) -> usize {
    let mut index = 0;
    while index < CANONICAL_EXTENSION_ORDER.len() {
        if CANONICAL_EXTENSION_ORDER[index] == letter {
            return index;
        }
        index += 1;
    }

    CANONICAL_EXTENSION_ORDER.len()
}

/// Key for sorting extension names in canonical order: single-letter extensions, then `Z`
/// extensions ordered by their category and name, then `S` extensions and everything else in
/// alphabetical order
const fn canonical_key(name: &[u8]) -> (u8, usize) {
    match *name {
        [letter] => (0, category(letter)),
        [b'z', letter, ..] => (1, category(letter)),
        [b's', ..] => (2, 0),
        _ => (3, 0),
    }
}

/// Compare extension names according to their canonical order
const fn canonical_cmp(a: &str, b: &str) -> Ordering {
    let a = a.as_bytes();
    let b = b.as_bytes();

    let (a_class, a_category) = canonical_key(a);
    let (b_class, b_category) = canonical_key(b);
    if a_class != b_class {
        return if a_class < b_class {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if a_category != b_category {
        return if a_category < b_category {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    let mut index = 0;
    while index < a.len() && index < b.len() {
        if a[index] != b[index] {
            return if a[index] < b[index] {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        index += 1;
    }

    if a.len() < b.len() {
        Ordering::Less
    } else if a.len() > b.len() {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}
