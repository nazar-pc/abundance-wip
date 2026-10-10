use crate::variable_bytes::VariableBytes;
use core::mem::MaybeUninit;

#[test]
fn from_buffer() {
    let buffer = [1, 2, 3, 4];

    {
        let size = 4;
        let bytes = VariableBytes::<0>::from_buffer(&buffer, &size).unwrap();
        assert_eq!(bytes.capacity(), 4);
        assert_eq!(bytes.get_initialized(), &[1, 2, 3, 4]);
    }

    // Size larger than the buffer, bytes beyond the buffer would be exposed otherwise
    {
        let size = 4;
        let bytes = VariableBytes::<0>::from_buffer(&buffer[..2], &size);
        assert_eq!(bytes.as_deref().map(VariableBytes::get_initialized), None);
    }

    // Size smaller than the buffer
    {
        let size = 2;
        let bytes = VariableBytes::<0>::from_buffer(&buffer, &size);
        assert_eq!(bytes.as_deref().map(VariableBytes::get_initialized), None);
    }
}

#[test]
fn from_buffer_mut() {
    let mut buffer = [1, 2, 3, 4];

    {
        let mut size = 4;
        let mut bytes = VariableBytes::<0>::from_buffer_mut(&mut buffer, &mut size).unwrap();
        assert_eq!(bytes.capacity(), 4);
        assert_eq!(bytes.get_initialized_mut(), &mut [1, 2, 3, 4]);
    }

    // Size larger than the buffer, bytes beyond the buffer would be exposed otherwise
    {
        let mut size = 4;
        let mut bytes = VariableBytes::<0>::from_buffer_mut(&mut buffer[..2], &mut size);
        assert_eq!(
            bytes.as_deref_mut().map(VariableBytes::get_initialized_mut),
            None
        );
    }

    // Size smaller than the buffer
    {
        let mut size = 2;
        let mut bytes = VariableBytes::<0>::from_buffer_mut(&mut buffer, &mut size);
        assert_eq!(
            bytes.as_deref_mut().map(VariableBytes::get_initialized_mut),
            None
        );
    }
}

#[test]
fn from_uninit() {
    let mut buffer = [MaybeUninit::uninit(); 4];

    {
        let mut size = 0;
        let bytes = VariableBytes::<0>::from_uninit(&mut buffer, &mut size).unwrap();
        assert_eq!(bytes.capacity(), 4);
        assert_eq!(bytes.get_initialized(), &[]);
    }

    // Non-zero size, uninitialized bytes would be exposed as initialized otherwise
    {
        let mut size = 2;
        let bytes = VariableBytes::<0>::from_uninit(&mut buffer, &mut size);
        assert_eq!(bytes.as_deref().map(VariableBytes::get_initialized), None);
    }
}

#[test]
fn append() {
    let mut buffer = [MaybeUninit::uninit(); 4];
    let mut size = 0;
    let mut bytes = VariableBytes::<0>::from_uninit(&mut buffer, &mut size).unwrap();

    assert!(bytes.append(&[1, 2]));
    assert_eq!(bytes.get_initialized(), &[1, 2]);

    // Not enough capacity
    assert!(!bytes.append(&[3, 4, 5]));
    assert_eq!(bytes.get_initialized(), &[1, 2]);

    assert!(bytes.append(&[3, 4]));
    assert_eq!(bytes.get_initialized(), &[1, 2, 3, 4]);
}
