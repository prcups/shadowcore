mod r#box;
mod cow;
mod option;
mod primitive;
mod slice;
mod string;
mod vec;

#[cfg(test)]
mod tests {
    use crate::foreign::*;

    use std::ffi::{c_char, CStr, CString};
    use std::marker::PhantomData;

    pub struct StructWithLifetime<'a>(*const c_char, PhantomData<&'a CStr>);

    impl FreeForeign for StructWithLifetime<'_> {
        type Foreign = *const c_char;

        unsafe fn free_foreign(_ptr: *mut Self::Foreign) {
            panic!("should not be reached");
        }
    }

    impl<'a> StructWithLifetime<'a> {
        fn new(bp: &BorrowedPointer<CStr, &'a CStr>) -> Self {
            StructWithLifetime(bp.as_ptr(), PhantomData)
        }

        fn as_ptr(&self) -> *const *const c_char {
            &self.0
        }
    }

    impl<'a> BorrowForeign<'a> for StructWithLifetime<'a> {
        type Storage = &'a Self;

        fn borrow_foreign(&'a self) -> BorrowedPointer<Self, &'a Self> {
            // SAFETY: data behind a shared reference cannot move
            // as long as the reference is alive
            unsafe { BorrowedPointer::new(&self.0, self) }
        }
    }

    impl<'a> BorrowForeignMut<'a> for StructWithLifetime<'a> {
        type Storage = &'a mut Self;

        fn borrow_foreign_mut(&'a mut self) -> BorrowedMutPointer<Self, &'a mut Self> {
            // SAFETY: data behind a shared reference cannot move
            // as long as the reference is alive
            unsafe { BorrowedMutPointer::new(&mut self.0, self) }
        }
    }

    #[test]
    fn test_borrow_struct() {
        let s = CString::new("hello world").unwrap();
        let mut st = StructWithLifetime::new(&s.borrow_foreign());

        // does not compile:
        // drop(s);

        let b = st.borrow_foreign();
        let p = st.as_ptr();
        assert_eq!(b.as_ptr(), p);
        assert_eq!(st.borrow_foreign_mut().as_ptr(), p);

        // does not compile:
        // assert_eq!(b.as_ptr(), p);
    }

    #[test]
    fn test_borrow_box() {
        let s = CString::new("hello world").unwrap();
        let st = StructWithLifetime::new(&s.borrow_foreign());
        let mut st = Box::new(st);

        // does not compile:
        // drop(s);

        let b = st.borrow_foreign();
        let p = st.as_ptr();
        assert_eq!(b.as_ptr(), p);
        assert_eq!(st.borrow_foreign_mut().as_ptr(), p);

        // does not compile:
        // assert_eq!(b.as_ptr(), p);
    }
}
