use crate::foreign::*;

impl<T> FreeForeign for Vec<T>
where
    [T]: FreeForeign,
{
    type Foreign = <[T] as FreeForeign>::Foreign;

    unsafe fn free_foreign(x: *mut Self::Foreign) {
        <[T]>::free_foreign(x)
    }
}

impl<T> CloneToForeign for Vec<T>
where
    [T]: CloneToForeign,
{
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        (**self).clone_to_foreign().into()
    }
}

impl<'a, T> BorrowForeign<'a> for Vec<T>
where
    [T]: BorrowForeign<'a>,
{
    type Storage = <[T] as BorrowForeign<'a>>::Storage;

    fn borrow_foreign(&'a self) -> BorrowedPointer<Self, Self::Storage> {
        (**self).borrow_foreign().into()
    }
}

impl<T> IntoForeign for Vec<T>
where
    [T]: for<'a> BorrowForeignMut<'a, Storage = &'a mut [T]> + 'static,
{
    type Storage = Self;

    fn into_foreign(mut self) -> BorrowedMutPointer<Self, Self> {
        let p = self.borrow_foreign_mut().as_mut_ptr();
        // SAFETY: The pointer remains valid because the Vec always dereferences
        // to the same value
        unsafe { BorrowedMutPointer::new(p, self) }
    }
}

impl<'a, T> BorrowForeignMut<'a> for Vec<T>
where
    [T]: BorrowForeignMut<'a>,
{
    type Storage = <[T] as BorrowForeignMut<'a>>::Storage;

    fn borrow_foreign_mut(&'a mut self) -> BorrowedMutPointer<Self, Self::Storage> {
        (**self).borrow_foreign_mut().into()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr;

    use crate::foreign::*;

    #[test]
    fn test_vec() {
        // A vec can be produced if a slice type for the elements has the capability.
        let v: Vec<u8> = vec![1, 2, 3];
        let bstr = b"\x01\x02\x03";
        let cloned = v.clone_to_foreign();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    cloned.as_ptr().cast::<c_void>(),
                    bstr.as_ptr().cast::<c_void>(),
                    bstr.len()
                ),
                0
            );
        }
    }

    #[test]
    fn test_vec_borrow() {
        // A vec can be produced if a slice type for the elements has the capability.
        let v: Vec<u8> = vec![0x31, 0x41, 0x59];
        let bstr = b"\x31\x41\x59";
        let borrowed = v.borrow_foreign();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    borrowed.as_ptr().cast::<c_void>(),
                    bstr.as_ptr().cast::<c_void>(),
                    bstr.len()
                ),
                0
            );
        }
    }

    #[test]
    fn test_vec_into_foreign() {
        // A vec can be produced if a slice type for the elements can be borrowed.
        let v: Vec<u8> = vec![0x21, 0x78, 0x28];
        let bstr = b"\x21\x78\x28";
        let consumed = v.into_foreign();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    consumed.as_ptr().cast::<c_void>(),
                    bstr.as_ptr().cast::<c_void>(),
                    bstr.len()
                ),
                0
            );
        }
    }

    #[test]
    fn test_vec_borrow_mut() {
        // A vec can be produced if a slice type for the elements has the capability.
        let mut v: Vec<u8> = vec![0x57, 0x72, 0x15];
        let bstr1 = b"\x57\x72\x15";
        let mut borrowed = v.borrow_foreign_mut();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    borrowed.as_ptr().cast::<c_void>(),
                    bstr1.as_ptr().cast::<c_void>(),
                    bstr1.len()
                ),
                0
            );
            ptr::write(borrowed.as_mut_ptr().offset(1), 0xFF);
        }
        assert_eq!(v[1], 0xFF);
    }
}
