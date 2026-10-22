use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::ManuallyDrop;
use std::ptr;

use crate::foreign::*;

impl FreeForeign for str {
    type Foreign = c_char;

    unsafe fn free_foreign(ptr: *mut c_char) {
        libc::free(ptr.cast::<c_void>());
    }
}

impl CloneToForeign for str {
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        // SAFETY: self.as_ptr() is guaranteed to point to self.len() bytes;
        // the destination is freshly allocated
        unsafe {
            let p = libc::malloc(self.len() + 1).cast::<c_char>();
            ptr::copy_nonoverlapping(self.as_ptr().cast::<c_char>(), p, self.len());
            *p.add(self.len()) = 0;
            OwnedPointer::new(p)
        }
    }
}

impl FreeForeign for CStr {
    type Foreign = c_char;

    unsafe fn free_foreign(ptr: *mut c_char) {
        libc::free(ptr.cast::<c_void>());
    }
}

impl CloneToForeign for CStr {
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        // SAFETY: self.as_ptr() is guaranteed to point to self.len() bytes;
        // the destination is freshly allocated
        unsafe {
            let slice = self.to_bytes_with_nul();
            let p = libc::malloc(slice.len()).cast::<c_char>();
            ptr::copy_nonoverlapping(self.as_ptr().cast::<c_char>(), p, slice.len());
            OwnedPointer::new(p)
        }
    }
}

impl<'a> BorrowForeign<'a> for CStr {
    type Storage = &'a CStr;

    fn borrow_foreign(&self) -> BorrowedPointer<Self, &CStr> {
        // SAFETY: a CStr is a stable pointer
        unsafe { BorrowedPointer::new(self.as_ptr(), self) }
    }
}

impl FreeForeign for String {
    type Foreign = c_char;

    unsafe fn free_foreign(ptr: *mut c_char) {
        libc::free(ptr.cast::<c_void>());
    }
}

impl CloneToForeign for String {
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        self.as_str().clone_to_foreign().into()
    }
}

impl FromForeign for String {
    unsafe fn cloned_from_foreign(p: *const c_char) -> Self {
        let cstr = CStr::from_ptr(p);
        String::from_utf8_lossy(cstr.to_bytes()).into_owned()
    }
}

impl IntoForeign for String {
    type Storage = Vec<c_char>;

    fn into_foreign(self) -> BorrowedMutPointer<Self, Vec<c_char>> {
        CString::new(self).unwrap().into_foreign().into()
    }
}

impl IntoForeign for CString {
    type Storage = Vec<c_char>;

    fn into_foreign(self) -> BorrowedMutPointer<Self, Vec<c_char>> {
        let bytes = self.into_bytes_with_nul();

        // Change u8 to c_char.
        let mut bytes = ManuallyDrop::new(bytes);
        let (ptr, length, capacity) = (bytes.as_mut_ptr(), bytes.len(), bytes.capacity());
        let bytes: Vec<c_char> = unsafe { Vec::from_raw_parts(ptr.cast(), length, capacity) };
        bytes.into_foreign().into()
    }
}

impl FreeForeign for CString {
    type Foreign = c_char;

    unsafe fn free_foreign(ptr: *mut c_char) {
        libc::free(ptr.cast::<c_void>());
    }
}

impl CloneToForeign for CString {
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        self.as_c_str().clone_to_foreign().into()
    }
}

impl FromForeign for CString {
    unsafe fn cloned_from_foreign(p: *const c_char) -> Self {
        CStr::from_ptr(p).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{c_char, c_void, CStr, CString};

    use crate::c_str::c_str;
    use crate::foreign::*;

    #[test]
    fn test_cloned_from_foreign_string() {
        let s = "Hello, world!".to_string();
        let cstr = c_str!("Hello, world!");
        let cloned = unsafe { String::cloned_from_foreign(cstr.as_ptr()) };
        assert_eq!(s, cloned);
    }

    #[test]
    fn test_cloned_from_foreign_cstring() {
        let s = CString::new("Hello, world!").unwrap();
        let cloned = s.clone_to_foreign();
        let copy = unsafe { CString::cloned_from_foreign(cloned.as_ptr()) };
        assert_ne!(copy.as_ptr(), cloned.as_ptr());
        assert_ne!(copy.as_ptr(), s.as_ptr());
        assert_eq!(copy, s);
    }

    #[test]
    fn test_from_foreign_string() {
        let s = "Hello, world!".to_string();
        let cloned = s.clone_to_foreign_ptr();
        let copy = unsafe { String::from_foreign(cloned) };
        assert_eq!(s, copy);
    }

    #[test]
    fn test_owned_pointer_into() {
        let s = "Hello, world!";
        let cloned: OwnedPointer<String> = s.clone_to_foreign().into();
        let copy = cloned.into_native();
        assert_eq!(s, copy);
    }

    #[test]
    fn test_owned_pointer_into_native() {
        let s = "Hello, world!".to_string();
        let cloned = s.clone_to_foreign();
        let copy = cloned.into_native();
        assert_eq!(s, copy);
    }

    #[test]
    fn test_ptr_into_native() {
        let s = "Hello, world!".to_string();
        let cloned = s.clone_to_foreign_ptr();
        let copy: String = unsafe { cloned.into_native() };
        assert_eq!(s, copy);

        // This is why type bounds are needed... they aren't for
        // OwnedPointer::into_native
        let cloned = s.clone_to_foreign_ptr();
        let copy: c_char = unsafe { cloned.into_native() };
        assert_eq!(s.as_bytes()[0], copy as u8);
    }

    #[test]
    fn test_clone_to_foreign_str() {
        let s = "Hello, world!";
        let p = c_str!("Hello, world!").as_ptr();
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

    #[test]
    fn test_into_foreign_cstring() {
        let s = c_str!("Hello, world!").to_owned();
        let p = c_str!("Hello, world!");
        let mut consumed = s.into_foreign();
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

            *consumed.as_mut_ptr().offset(5) = 0;
            assert_eq!(String::cloned_from_foreign(consumed.as_ptr()), "Hello");
        }
    }

    #[test]
    fn test_into_foreign_string() {
        let s = "Hello, world!".to_string();
        let p = c_str!("Hello, world!");
        let mut consumed = s.into_foreign();
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

            *consumed.as_mut_ptr().offset(5) = 0;
            assert_eq!(String::cloned_from_foreign(consumed.as_ptr()), "Hello");
        }
    }

    #[test]
    fn test_clone_to_foreign_cstr() {
        let s: &CStr = c_str!("Hello, world!");
        let cloned = s.clone_to_foreign();
        unsafe {
            let len = libc::strlen(cloned.as_ptr());
            assert_eq!(len, s.to_bytes().len());
            assert_eq!(
                libc::memcmp(
                    cloned.as_ptr().cast::<c_void>(),
                    s.as_ptr().cast::<c_void>(),
                    len + 1
                ),
                0
            );
        }
    }

    #[test]
    fn test_clone_to_foreign_bytes() {
        let s = b"Hello, world!\0";
        let cloned = s.clone_to_foreign();
        unsafe {
            let len = libc::strlen(cloned.as_ptr().cast::<c_char>());
            assert_eq!(len, s.len() - 1);
            assert_eq!(
                libc::memcmp(
                    cloned.as_ptr().cast::<c_void>(),
                    s.as_ptr().cast::<c_void>(),
                    len + 1
                ),
                0
            );
        }
    }

    #[test]
    fn test_clone_to_foreign_cstring() {
        let s = CString::new("Hello, world!").unwrap();
        let cloned = s.clone_to_foreign();
        unsafe {
            let len = libc::strlen(cloned.as_ptr());
            assert_eq!(len, s.to_bytes().len());
            assert_ne!(s.as_ptr(), cloned.as_ptr());
            assert_eq!(
                libc::memcmp(
                    cloned.as_ptr().cast::<c_void>(),
                    s.as_ptr().cast::<c_void>(),
                    len + 1
                ),
                0
            );
        }
    }

    #[test]
    fn test_clone_to_foreign_string() {
        let s = "Hello, world!".to_string();
        let cstr = c_str!("Hello, world!");
        let cloned = s.clone_to_foreign();
        unsafe {
            let len = libc::strlen(cloned.as_ptr());
            assert_eq!(len, s.len());
            assert_eq!(
                libc::memcmp(
                    cloned.as_ptr().cast::<c_void>(),
                    cstr.as_ptr().cast::<c_void>(),
                    len + 1
                ),
                0
            );
        }
    }
}
