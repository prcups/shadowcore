use std::ptr;

use crate::foreign::*;

impl<T> FreeForeign for Option<T>
where
    T: FreeForeign,
{
    type Foreign = <T as FreeForeign>::Foreign;

    unsafe fn free_foreign(x: *mut Self::Foreign) {
        T::free_foreign(x)
    }
}

impl<T> CloneToForeign for Option<T>
where
    T: CloneToForeign,
{
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        // Same as the underlying implementation, but also convert `None`
        // to a `NULL` pointer.
        self.as_ref()
            .map(CloneToForeign::clone_to_foreign)
            .map(OwnedPointer::into)
            .unwrap_or(OwnedPointer::null_mut())
    }
}

impl<T> FromForeign for Option<T>
where
    T: FromForeign,
{
    unsafe fn cloned_from_foreign(p: *const Self::Foreign) -> Self {
        // Same as the underlying implementation, but also accept a `NULL` pointer.
        if p.is_null() {
            None
        } else {
            Some(T::cloned_from_foreign(p))
        }
    }
}

impl<'a, T> BorrowForeign<'a> for Option<T>
where
    T: BorrowForeign<'a>,
{
    type Storage = Option<<T as BorrowForeign<'a>>::Storage>;

    fn borrow_foreign(&'a self) -> BorrowedPointer<Self, Self::Storage> {
        match self.as_ref().map(BorrowForeign::borrow_foreign) {
            // SAFETY: a NULL pointer does not point into the storage
            None => unsafe { BorrowedPointer::new(ptr::null(), None) },
            Some(bp) => bp.map(Some),
        }
    }
}

impl<T> IntoForeign for Option<T>
where
    T: IntoForeign,
{
    type Storage = Option<<T as IntoForeign>::Storage>;

    fn into_foreign(self) -> BorrowedMutPointer<Self, Self::Storage> {
        match self.map(IntoForeign::into_foreign) {
            // SAFETY: a NULL pointer does not point into the storage
            None => unsafe { BorrowedMutPointer::new(ptr::null_mut(), None) },
            Some(bp) => bp.map(Some),
        }
    }
}

impl<'a, T> BorrowForeignMut<'a> for Option<T>
where
    T: BorrowForeignMut<'a>,
{
    type Storage = Option<<T as BorrowForeignMut<'a>>::Storage>;

    fn borrow_foreign_mut(&'a mut self) -> BorrowedMutPointer<Self, Self::Storage> {
        match self.as_mut().map(BorrowForeignMut::borrow_foreign_mut) {
            // SAFETY: a NULL pointer does not point into the storage
            None => unsafe { BorrowedMutPointer::new(ptr::null_mut(), None) },
            Some(bp) => bp.map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::ffi::{c_void, CStr};
    use std::ptr;

    use crate::c_str::c_str;
    use crate::foreign::*;

    #[test]
    fn test_option() {
        // An Option can be produced if the inner type has the capability.
        let s = Some("Hello, world!".to_string());
        let cstr = c_str!("Hello, world!");
        unsafe {
            assert_eq!(Option::<String>::cloned_from_foreign(cstr.as_ptr()), s);
        }

        let s = Some(Box::new(128u16));
        let cloned = unsafe { Option::<Box<u16>>::cloned_from_foreign(&128u16) };
        assert_eq!(s, cloned);
    }

    #[test]
    fn test_option_none() {
        // An Option can be used to produce or convert NULL pointers
        unsafe {
            assert!(Option::<u16>::cloned_from_foreign(ptr::null()).is_none());
            assert!(Option::<u16>::from_foreign(ptr::null_mut()).is_none());
        }

        assert_eq!(
            Option::<String>::into_foreign(None).as_ptr(),
            ptr::null_mut()
        );

        assert_eq!(
            Option::<String>::clone_to_foreign_ptr(&None),
            ptr::null_mut()
        );
    }

    #[test]
    fn test_option_into_foreign() {
        // An Option can be used to produce or convert NULL pointers
        let s = Some("Hello, world!".to_string());
        let p = c_str!("Hello, world!");
        let consumed = s.into_foreign();
        unsafe {
            let len = libc::strlen(consumed.as_ptr());
            assert_eq!(len, p.to_bytes().len());
            assert_eq!(
                libc::memcmp(
                    consumed.as_ptr().cast::<c_void>(),
                    p.as_ptr().cast::<c_void>(),
                    len + 1
                ),
                0
            );
        }
    }

    #[test]
    fn test_option_borrow() {
        let s: Option<Cow<'static, CStr>> = None;
        assert_eq!(s.borrow_foreign().as_ptr(), ptr::null());
    }

    #[test]
    fn test_option_borrow_mut() {
        let mut s: Option<i8> = None;
        assert_eq!(s.borrow_foreign_mut().as_ptr(), ptr::null_mut());
    }
}
