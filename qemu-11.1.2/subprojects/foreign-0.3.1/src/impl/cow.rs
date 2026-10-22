use std::borrow::Cow;

use crate::foreign::*;

impl<'a, B> FreeForeign for Cow<'a, B>
where
    B: 'a + ToOwned + ?Sized + FreeForeign,
{
    type Foreign = B::Foreign;

    unsafe fn free_foreign(ptr: *mut B::Foreign) {
        B::free_foreign(ptr);
    }
}

impl<'a, B> CloneToForeign for Cow<'a, B>
where
    B: 'a + ToOwned + ?Sized + CloneToForeign,
{
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        (**self).clone_to_foreign().into()
    }
}

impl<'a, B> BorrowForeign<'a> for Cow<'a, B>
where
    B: 'a + ToOwned + ?Sized + BorrowForeign<'a>,
{
    type Storage = B::Storage;

    fn borrow_foreign(&'a self) -> BorrowedPointer<Self, B::Storage> {
        (**self).borrow_foreign().into()
    }
}

impl<'a, B> FromForeign for Cow<'a, B>
where
    B: 'a + ToOwned + ?Sized + FreeForeign,
    <B as ToOwned>::Owned: FromForeign + FreeForeign<Foreign = B::Foreign>,
{
    unsafe fn cloned_from_foreign(ptr: *const B::Foreign) -> Self {
        let cloned: <B as ToOwned>::Owned = FromForeign::cloned_from_foreign(ptr);
        Cow::Owned(cloned)
    }
}

impl<'a, B> IntoForeign for Cow<'a, B>
where
    B: 'a + ToOwned + ?Sized + FreeForeign,
    <B as ToOwned>::Owned: IntoForeign + FreeForeign<Foreign = B::Foreign>,
{
    type Storage = <B::Owned as IntoForeign>::Storage;

    fn into_foreign(self) -> BorrowedMutPointer<Self, Self::Storage> {
        self.into_owned().into_foreign().into()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::ffi::{c_void, CStr};

    use crate::c_str::c_str;
    use crate::foreign::*;

    #[test]
    fn test_cloned_from_foreign_string_cow() {
        let s = "Hello, world!".to_string();
        let cstr = c_str!("Hello, world!");
        let cloned = unsafe { Cow::cloned_from_foreign(cstr.as_ptr()) };
        assert_eq!(s, cloned);
    }

    #[test]
    fn test_clone_to_foreign_string_cow() {
        let p = c_str!("Hello, world!").as_ptr();
        for s in [
            Into::<Cow<str>>::into("Hello, world!"),
            Into::<Cow<str>>::into("Hello, world!".to_string()),
        ] {
            let cloned = s.clone_to_foreign();
            unsafe {
                let len = libc::strlen(cloned.as_ptr());
                assert_eq!(len, s.len());
                assert_eq!(
                    libc::memcmp(
                        cloned.as_ptr().cast::<c_void>(),
                        p.cast::<c_void>(),
                        len + 1
                    ),
                    0
                );
            }
        }
    }

    #[test]
    fn test_borrow_foreign_string_cow() {
        let p = c_str!("Hello, world!").as_ptr();
        for s in [
            Into::<Cow<CStr>>::into(c_str!("Hello, world!")),
            Into::<Cow<CStr>>::into(c_str!("Hello, world!").to_owned()),
        ] {
            let borrowed = s.borrow_foreign();
            unsafe {
                let len = libc::strlen(borrowed.as_ptr());
                assert_eq!(len, libc::strlen(p));
                assert_eq!(
                    libc::memcmp(
                        borrowed.as_ptr().cast::<c_void>(),
                        p.cast::<c_void>(),
                        len + 1
                    ),
                    0
                );
            }
        }
    }

    #[test]
    fn test_into_foreign_string_cow() {
        let p = c_str!("Hello, world!").as_ptr();
        for s in [
            Into::<Cow<str>>::into("Hello, world!"),
            Into::<Cow<str>>::into("Hello, world!".to_string()),
        ] {
            let consumed = s.into_foreign();
            unsafe {
                let len = libc::strlen(consumed.as_ptr());
                assert_eq!(len, libc::strlen(p));
                assert_eq!(
                    libc::memcmp(
                        consumed.as_ptr().cast::<c_void>(),
                        p.cast::<c_void>(),
                        len + 1
                    ),
                    0
                );
            }
        }
    }
}
