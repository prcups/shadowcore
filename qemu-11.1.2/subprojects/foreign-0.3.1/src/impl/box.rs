use crate::foreign::*;

impl<T: ?Sized> FreeForeign for Box<T>
where
    T: FreeForeign,
{
    type Foreign = <T as FreeForeign>::Foreign;

    unsafe fn free_foreign(x: *mut Self::Foreign) {
        T::free_foreign(x)
    }
}

impl<T: ?Sized> CloneToForeign for Box<T>
where
    T: CloneToForeign,
{
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        self.as_ref().clone_to_foreign().into()
    }
}

impl<T> FromForeign for Box<T>
where
    T: FromForeign,
{
    unsafe fn cloned_from_foreign(p: *const Self::Foreign) -> Self {
        Box::new(T::cloned_from_foreign(p))
    }
}

impl<'a, T: ?Sized> BorrowForeign<'a> for Box<T>
where
    T: BorrowForeign<'a>,
{
    type Storage = <T as BorrowForeign<'a>>::Storage;

    fn borrow_foreign(&'a self) -> BorrowedPointer<Self, Self::Storage> {
        self.as_ref().borrow_foreign().into()
    }
}

impl<T: ?Sized> IntoForeign for Box<T>
where
    T: for<'a> BorrowForeignMut<'a, Storage = &'a mut T> + 'static,
{
    type Storage = Self;

    fn into_foreign(mut self) -> BorrowedMutPointer<Self, Self> {
        let p = self.borrow_foreign_mut().as_mut_ptr();
        // SAFETY: The pointer remains valid because the Box always dereferences
        // to the same value
        unsafe { BorrowedMutPointer::new(p, self) }
    }
}

impl<'a, T: ?Sized> BorrowForeignMut<'a> for Box<T>
where
    T: BorrowForeignMut<'a>,
{
    type Storage = <T as BorrowForeignMut<'a>>::Storage;

    fn borrow_foreign_mut(&'a mut self) -> BorrowedMutPointer<Self, Self::Storage> {
        self.as_mut().borrow_foreign_mut().into()
    }
}

#[cfg(test)]
mod tests {
    use crate::c_str::c_str;
    use crate::foreign::*;

    use std::ffi::{c_char, c_void};

    #[test]
    fn test_box_cloned_from_foreign() {
        // A box can be produced if the inner type has the capability.
        let s = Box::new("Hello, world!".to_string());
        let cstr = c_str!("Hello, world!");
        let cloned = unsafe { Box::<String>::cloned_from_foreign(cstr.as_ptr()) };
        assert_eq!(s, cloned);
    }

    #[test]
    fn test_box_clone_to_foreign() {
        // A box can be turned into a foreign object if the inner type has the capability.
        let s = Box::new(128u32);
        let cloned = s.clone_to_foreign();
        assert_eq!(*s, unsafe { *(cloned.as_ptr()) });
    }

    #[test]
    fn test_box_into_foreign() {
        // A box can be consumed if the inner type can be borrowed.
        let s = Box::new([128i32, 256i32]);
        let consumed = s.into_foreign();
        assert_eq!(unsafe { *consumed.as_ptr() }, 128);
        assert_eq!(unsafe { *consumed.as_ptr().offset(1) }, 256);
    }

    #[test]
    fn test_box_borrow() {
        // Contents of a Box can be borrowed.
        let s = Box::new(c_str!("Hello, world!"));
        let borrowed = s.borrow_foreign();
        let cloned = unsafe { Box::<String>::cloned_from_foreign(borrowed.as_ptr()) };
        assert_eq!(s.to_str().unwrap(), *cloned);
    }

    #[test]
    fn test_box_borrow_mut() {
        // Contents of a Box can be borrowed.
        let mut s = Box::new(123u16);
        let mut borrowed = s.borrow_foreign_mut();
        unsafe { *borrowed.as_mut_ptr() = 456 };
        assert_eq!(*s, 456);
    }

    #[test]
    fn test_box_unsized() {
        let original = c_str!("Hello, world!");
        let boxed = original.to_owned().into_bytes_with_nul().into_boxed_slice();
        let borrowed = boxed.borrow_foreign();
        unsafe {
            let len = libc::strlen(borrowed.as_ptr().cast::<c_char>());
            assert_eq!(len, original.to_bytes().len());
            assert_eq!(
                libc::memcmp(
                    borrowed.as_ptr().cast::<c_void>(),
                    original.as_ptr().cast::<c_void>(),
                    len + 1
                ),
                0
            );
        }
    }
}
