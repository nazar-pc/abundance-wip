use crate::fixed_capacity_bytes::{FixedCapacityBytesU8, FixedCapacityBytesU16};

#[test]
fn append_u8() {
    let mut bytes = FixedCapacityBytesU8::<4>::try_from_bytes(&[1]).unwrap();

    assert!(bytes.append(&[2, 3]));
    assert_eq!(bytes.get_bytes(), &[1, 2, 3]);

    // Not enough capacity
    assert!(!bytes.append(&[4, 5]));
    assert_eq!(bytes.get_bytes(), &[1, 2, 3]);

    assert!(bytes.append(&[4]));
    assert_eq!(bytes.get_bytes(), &[1, 2, 3, 4]);
}

#[test]
fn append_u16() {
    let mut bytes = FixedCapacityBytesU16::<4>::try_from_bytes(&[1]).unwrap();

    assert!(bytes.append(&[2, 3]));
    assert_eq!(bytes.get_bytes(), &[1, 2, 3]);

    // Not enough capacity
    assert!(!bytes.append(&[4, 5]));
    assert_eq!(bytes.get_bytes(), &[1, 2, 3]);

    assert!(bytes.append(&[4]));
    assert_eq!(bytes.get_bytes(), &[1, 2, 3, 4]);
}

#[test]
fn copy_from_u8() {
    let mut bytes = FixedCapacityBytesU8::<4>::try_from_bytes(&[1]).unwrap();

    assert!(bytes.copy_from(&[2, 3]));
    assert_eq!(bytes.get_bytes(), &[2, 3]);

    // Not enough capacity
    assert!(!bytes.copy_from(&[4, 5, 6, 7, 8]));
    assert_eq!(bytes.get_bytes(), &[2, 3]);

    assert!(bytes.copy_from(&[4, 5, 6, 7]));
    assert_eq!(bytes.get_bytes(), &[4, 5, 6, 7]);
}

#[test]
fn copy_from_u16() {
    let mut bytes = FixedCapacityBytesU16::<4>::try_from_bytes(&[1]).unwrap();

    assert!(bytes.copy_from(&[2, 3]));
    assert_eq!(bytes.get_bytes(), &[2, 3]);

    // Not enough capacity
    assert!(!bytes.copy_from(&[4, 5, 6, 7, 8]));
    assert_eq!(bytes.get_bytes(), &[2, 3]);

    assert!(bytes.copy_from(&[4, 5, 6, 7]));
    assert_eq!(bytes.get_bytes(), &[4, 5, 6, 7]);
}
