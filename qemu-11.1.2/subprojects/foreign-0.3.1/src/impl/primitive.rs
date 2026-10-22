use std::ffi::c_void;
use std::mem::{self, MaybeUninit};
use std::ptr;

use crate::foreign::*;

macro_rules! foreign_copy_type {
    ($rust_type:ty, $foreign_type:ty) => {
        impl FreeForeign for $rust_type {
            type Foreign = $foreign_type;

            unsafe fn free_foreign(ptr: *mut Self::Foreign) {
                libc::free(ptr.cast::<c_void>());
            }
        }

        impl CloneToForeign for $rust_type {
            fn clone_to_foreign(&self) -> OwnedPointer<Self> {
                // Safety: we are copying into a freshly-allocated block
                unsafe {
                    let p = libc::malloc(mem::size_of::<Self>()).cast::<Self::Foreign>();
                    *p = *self as Self::Foreign;
                    OwnedPointer::new(p)
                }
            }
        }

        impl FromForeign for $rust_type {
            unsafe fn cloned_from_foreign(p: *const Self::Foreign) -> Self {
                *p
            }
        }

        impl<'a> BorrowForeign<'a> for $rust_type {
            type Storage = &'a Self;

            fn borrow_foreign(&self) -> BorrowedPointer<Self, &Self> {
                // SAFETY: data behind a shared reference cannot move
                // as long as the reference is alive
                unsafe { BorrowedPointer::new(self, self) }
            }
        }

        impl<'a> BorrowForeignMut<'a> for $rust_type {
            type Storage = &'a mut Self;

            fn borrow_foreign_mut(&mut self) -> BorrowedMutPointer<Self, &mut Self> {
                // SAFETY: the data behind the reference will not move as
                // long as the reference is alive
                unsafe { BorrowedMutPointer::new(self, self) }
            }
        }

        unsafe impl FixedAlloc for $rust_type {
            unsafe fn clone_into_foreign(dest: *mut Self::Foreign, src: &Self) {
                ptr::write(dest, *src)
            }

            unsafe fn clone_array_from_foreign<const N: usize>(
                src: *const Self::Foreign,
            ) -> [Self; N] {
                unsafe {
                    let mut uninit = MaybeUninit::<[Self; N]>::uninit();
                    ptr::copy_nonoverlapping(src, uninit.as_mut_ptr().cast::<Self::Foreign>(), N);
                    uninit.assume_init()
                }
            }

            unsafe fn clone_from_native_slice(dest: *mut Self::Foreign, src: &[Self]) {
                unsafe {
                    ptr::copy_nonoverlapping(src.as_ptr().cast::<Self::Foreign>(), dest, src.len());
                }
            }
        }
    };
}
foreign_copy_type!(bool, bool);
foreign_copy_type!(i8, i8);
foreign_copy_type!(u8, u8);
foreign_copy_type!(i16, i16);
foreign_copy_type!(u16, u16);
foreign_copy_type!(i32, i32);
foreign_copy_type!(u32, u32);
foreign_copy_type!(i64, i64);
foreign_copy_type!(u64, u64);
foreign_copy_type!(isize, libc::ptrdiff_t);
foreign_copy_type!(usize, libc::size_t);
foreign_copy_type!(f32, f32);
foreign_copy_type!(f64, f64);

#[cfg(test)]
mod tests {
    use crate::foreign::*;

    #[test]
    fn test_foreign_int_convert() {
        let i = 123i8;
        let p = i.clone_to_foreign();
        unsafe {
            assert_eq!(i, *p.as_ptr());
            assert_eq!(i, i8::cloned_from_foreign(p.as_ptr()));
        }
        assert_eq!(i, p.into_native());

        let p = i.clone_to_foreign();
        unsafe {
            assert_eq!(i, i8::from_foreign(p.into_inner()));
        }
    }

    #[test]
    fn test_foreign_int_borrow() {
        let i = 123i8;
        unsafe {
            assert_eq!(i, *i.borrow_foreign().as_ptr());
        }
        assert_eq!(i, 123i8);
    }

    #[test]
    fn test_foreign_int_borrow_mut() {
        let mut i = 123i8;
        let mut borrowed = i.borrow_foreign_mut();
        unsafe {
            assert_eq!(*borrowed.as_ptr(), 123i8);
            *borrowed.as_mut_ptr() = 45i8;
        }
        let borrowed = i.borrow_foreign_mut();
        unsafe {
            assert_eq!(*borrowed.as_ptr(), 45i8);
        }
        assert_eq!(i, 45i8);
    }
}
