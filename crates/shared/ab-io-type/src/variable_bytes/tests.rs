use crate::variable_bytes::VariableBytes;
use core::mem::MaybeUninit;

#[test]
fn append() {
    let mut buffer = [MaybeUninit::uninit(); 4];
    let mut size = 0;
    let mut bytes = VariableBytes::<0>::from_uninit(&mut buffer, &mut size);

    assert!(bytes.append(&[1, 2]));
    assert_eq!(bytes.get_initialized(), &[1, 2]);

    // Not enough capacity
    assert!(!bytes.append(&[3, 4, 5]));
    assert_eq!(bytes.get_initialized(), &[1, 2]);

    assert!(bytes.append(&[3, 4]));
    assert_eq!(bytes.get_initialized(), &[1, 2, 3, 4]);
}
