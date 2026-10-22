use std::ffi::c_void;
use std::mem;

use crate::foreign::*;

impl<T> FreeForeign for [T]
where
    T: FixedAlloc,
{
    type Foreign = T::Foreign;

    unsafe fn free_foreign(ptr: *mut Self::Foreign) {
        libc::free(ptr.cast::<c_void>());
    }
}

impl<T> CloneToForeign for [T]
where
    T: FixedAlloc,
{
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        // SAFETY: self.as_ptr() is guaranteed to point to the same number of bytes
        // as the freshly allocated destination
        unsafe {
            let size = mem::size_of::<Self::Foreign>();
            let p = libc::malloc(self.len() * size).cast::<Self::Foreign>();
            T::clone_from_native_slice(p, self);
            OwnedPointer::new(p)
        }
    }
}

impl<'a, T> BorrowForeign<'a> for [T]
where
    T: FixedAlloc + FreeForeign<Foreign = T> + BorrowForeign<'a, Foreign = T, Storage = &'a T> + 'a,
{
    type Storage = &'a Self;

    fn borrow_foreign(&self) -> BorrowedPointer<Self, &Self> {
        // SAFETY: data behind a shared reference cannot move
        // as long as the reference is alive
        unsafe { BorrowedPointer::new(self.as_ptr(), self) }
    }
}

impl<'a, T> BorrowForeignMut<'a> for [T]
where
    T: FixedAlloc
        + FreeForeign<Foreign = T>
        + BorrowForeignMut<'a, Foreign = T, Storage = &'a mut T>
        + 'a,
{
    type Storage = &'a mut Self;

    fn borrow_foreign_mut(&mut self) -> BorrowedMutPointer<Self, &mut Self> {
        // SAFETY: the data behind the reference will not move as
        // long as the reference is alive
        unsafe { BorrowedMutPointer::new(self.as_mut_ptr(), self) }
    }
}

impl<T, const N: usize> FreeForeign for [T; N]
where
    T: FixedAlloc,
{
    type Foreign = T::Foreign;

    unsafe fn free_foreign(ptr: *mut Self::Foreign) {
        libc::free(ptr.cast::<c_void>());
    }
}

impl<T, const N: usize> CloneToForeign for [T; N]
where
    T: FixedAlloc,
{
    fn clone_to_foreign(&self) -> OwnedPointer<Self> {
        self[..].clone_to_foreign().into()
    }
}

impl<T, const N: usize> FromForeign for [T; N]
where
    T: FixedAlloc,
{
    unsafe fn cloned_from_foreign(src: *const Self::Foreign) -> Self {
        T::clone_array_from_foreign(src)
    }
}

impl<'a, T, const N: usize> BorrowForeign<'a> for [T; N]
where
    T: FixedAlloc + BorrowForeign<'a, Foreign = T, Storage = &'a T> + 'a,
{
    type Storage = &'a [T; N];

    fn borrow_foreign(&'a self) -> BorrowedPointer<Self, &'a Self> {
        // SAFETY: data behind a shared reference cannot move
        // as long as the reference is alive
        unsafe { BorrowedPointer::new(self[..].as_ptr(), self) }
    }
}

impl<'a, T, const N: usize> BorrowForeignMut<'a> for [T; N]
where
    T: FixedAlloc + BorrowForeignMut<'a, Foreign = T, Storage = &'a mut T> + 'a,
{
    type Storage = &'a mut [T; N];

    fn borrow_foreign_mut(&'a mut self) -> BorrowedMutPointer<Self, &'a mut Self> {
        // SAFETY: data behind a shared reference cannot move
        // as long as the reference is alive
        unsafe { BorrowedMutPointer::new(self[..].as_mut_ptr(), self) }
    }
}

unsafe impl<T, const N: usize> FixedAlloc for [T; N]
where
    T: FixedAlloc,
{
    unsafe fn clone_into_foreign(dest: *mut Self::Foreign, src: &Self) {
        T::clone_from_native_slice(dest, &src[..]);
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;
    use std::ptr;

    use crate::foreign::*;

    #[test]
    fn test_slice_convert() {
        let a = [123i8, 45i8, 67i8];
        let i = &a[0..=1];
        let p = i.clone_to_foreign();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    i.as_ptr().cast::<c_void>(),
                    p.as_ptr().cast::<c_void>(),
                    i.len()
                ),
                0
            );
            assert_eq!(i, <[i8; 2]>::cloned_from_foreign(p.as_ptr()));
        }

        let p = i.clone_to_foreign();
        unsafe {
            assert_eq!(i, <[i8; 2]>::from_foreign(p.into_inner()));
        }
    }

    #[test]
    fn test_slice_borrow() {
        let i = [123i8, 45i8];
        let borrowed = i.borrow_foreign();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    i.as_ptr().cast::<c_void>(),
                    borrowed.as_ptr().cast::<c_void>(),
                    i.len()
                ),
                0
            );

            let cloned = <[i8; 2]>::cloned_from_foreign(borrowed.as_ptr());
            assert_eq!(i, cloned);
        }
    }

    #[test]
    fn test_slice_borrow_mut() {
        let mut i = [123u8, 45u8];
        let (p, len) = (i.as_ptr(), i.len());
        let mut borrowed = i.borrow_foreign_mut();
        unsafe {
            assert_eq!(
                libc::memcmp(p.cast::<c_void>(), borrowed.as_ptr().cast::<c_void>(), len),
                0
            );

            ptr::write(borrowed.as_mut_ptr().offset(1), 234);
            let cloned = <[u8; 2]>::cloned_from_foreign(borrowed.as_ptr());

            assert_eq!(i[1], 234);
            assert_eq!(i, cloned);
        }
    }

    #[test]
    fn test_array_convert() {
        let i = [123i8, 45i8];
        let p = i.clone_to_foreign();
        unsafe {
            assert_eq!(
                libc::memcmp(
                    i.as_ptr().cast::<c_void>(),
                    p.as_ptr().cast::<c_void>(),
                    i.len()
                ),
                0
            );
            assert_eq!(i, <[i8; 2]>::cloned_from_foreign(p.as_ptr()));
        }

        let p = i.clone_to_foreign();
        unsafe {
            assert_eq!(i, <[i8; 2]>::from_foreign(p.into_inner()));
        }
    }
}
