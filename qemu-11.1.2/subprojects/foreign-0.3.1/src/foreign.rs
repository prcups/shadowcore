/// Traits to map between C structs and native Rust types.
/// Similar to glib-rs but a bit simpler and possibly more
/// idiomatic.
use std::fmt;
use std::fmt::Debug;
use std::mem::{self, MaybeUninit};
use std::ptr;

/// A type for which there is a canonical representation as a C datum.
pub trait FreeForeign {
    /// The representation of `Self` as a C datum.  Typically a
    /// `struct`, though there are exceptions: for example `String`
    /// and `str` use `c_char` for strings, since C strings are of
    /// `char *` type.  These exceptional cases will not implement
    /// the `FixedAlloc` trait.
    type Foreign;

    /// Free the C datum pointed to by `p`.
    ///
    /// # Safety
    ///
    /// `p` must be `NULL` or point to valid data.
    ///
    /// ```
    /// # use foreign::{CloneToForeign, FreeForeign};
    /// let foreign = "Hello, world!".clone_to_foreign();
    /// unsafe {
    ///     String::free_foreign(foreign.into_inner());
    /// }
    /// ```
    unsafe fn free_foreign(p: *mut Self::Foreign);
}

/// A type for which a representation as a C datum can be produced.
pub trait CloneToForeign: FreeForeign {
    /// Convert a native Rust object to a foreign C pointer, copying
    /// everything pointed to by `self`; return an [`OwnedPointer`]
    /// which will take care of freeing the storage (same as
    /// `to_glib_none` in `glib-rs`)
    fn clone_to_foreign(&self) -> OwnedPointer<Self>;

    /// Convert a native Rust object to a foreign C pointer, copying
    /// everything pointed to by `self` (same as `to_glib_full`
    /// in `glib-rs`).
    ///
    /// This function is useful when C functions will take ownership
    /// of the returned pointer; alternatively, the pointer can
    /// be freed with the `free_foreign` associated function.
    #[must_use]
    fn clone_to_foreign_ptr(&self) -> *mut Self::Foreign {
        self.clone_to_foreign().into_inner()
    }
}

/// A type which is stored in a fixed amount of memory given by
/// `mem::size_of::<Self::Foreign>()`.
///
/// # Safety
///
/// Define this trait only if `CloneToForeign` allocates a single memory block,
/// of constant size `mem::size_of::<Self::Foreign>()`.
pub unsafe trait FixedAlloc: CloneToForeign + FromForeign + FreeForeign + Sized {
    /// Convert a native Rust object to a foreign C struct, copying its
    /// contents to preallocated memory at `dest`.
    ///
    /// ```
    /// # use std::mem::MaybeUninit;
    /// # use foreign::FixedAlloc;
    /// let mut dest = MaybeUninit::uninit();
    /// unsafe {
    ///     u8::clone_into_foreign(dest.as_mut_ptr(), &42);
    ///     assert_eq!(dest.assume_init(), 42);
    /// }
    /// ```
    ///
    /// # Safety
    ///
    /// `dest` must be allocated and writable.
    unsafe fn clone_into_foreign(dest: *mut Self::Foreign, src: &Self);

    /// Copy `src.len()` elements from memory at address `src` into a Rust array,
    /// using [`cloned_from_foreign`](FromForeign::cloned_from_foreign) on each
    /// element.
    ///
    /// ```
    /// # use std::mem::MaybeUninit;
    /// # use foreign::FixedAlloc;
    /// let src: [u8; 3] = [1, 2, 3];
    /// let dest: [u8; 3] = unsafe {
    ///     u8::clone_array_from_foreign(src.as_ptr())
    /// };
    /// assert_eq!(dest, src);
    /// ```
    ///
    /// # Safety
    ///
    /// `src` must be allocated, large enough to host `N` values of type
    /// `Self::Foreign`, and readable.
    unsafe fn clone_array_from_foreign<const N: usize>(mut src: *const Self::Foreign) -> [Self; N] {
        unsafe {
            // SAFETY: MaybeUninit<Self> has the same layout as Self
            let mut uninit = MaybeUninit::<[Self; N]>::uninit();

            let mut dest = uninit.as_mut_ptr().cast::<Self>();
            for _ in 0..N {
                ptr::write(dest, Self::cloned_from_foreign(src));
                dest = dest.offset(1);
                src = src.offset(1);
            }

            // SAFETY: each element was initialized with write()
            uninit.assume_init()
        }
    }

    /// Copy the `src.len()` elements of the slice into memory at address `dest`,
    /// using [`clone_into_foreign`](FixedAlloc::clone_into_foreign) on each
    /// element.
    ///
    /// ```
    /// # use std::mem::MaybeUninit;
    /// # use foreign::FixedAlloc;
    /// let mut dest: MaybeUninit::<[u8; 3]> = MaybeUninit::uninit();
    /// unsafe {
    ///     u8::clone_from_native_slice(dest.as_mut_ptr().cast(), &[1, 2, 3]);
    ///     assert_eq!(dest.assume_init(), [1, 2, 3]);
    /// };
    /// ```
    ///
    /// # Safety
    ///
    /// `dest` must be allocated, large enough to host `N` values of type
    /// `Self::Foreign`, and writable.
    unsafe fn clone_from_native_slice(mut dest: *mut Self::Foreign, src: &[Self]) {
        unsafe {
            for item in src {
                Self::clone_into_foreign(dest, item);
                dest = dest.offset(1);
            }
        }
    }
}

/// A type for C data that can be converted to native Rust object, taking ownership
/// of the C datum.  You should not need to implement this trait as long as the
/// Rust types implement `FromForeign`.
pub trait IntoNative<U> {
    /// Convert a C datum to a native Rust object, taking ownership of
    /// the pointer or Rust object (same as `from_glib_full` in `glib-rs`)
    ///
    /// # Safety
    ///
    /// `self` must point to valid data, or can be `NULL` if `U` is an
    /// `Option` type.  It becomes invalid after the function returns.
    ///
    /// ```
    /// # use foreign::{CloneToForeign, IntoNative};
    /// let s = "Hello, world!".to_string();
    /// let foreign = s.clone_to_foreign();
    /// let native: String = unsafe {
    ///     foreign.into_native()
    ///     // foreign is not leaked
    /// };
    /// assert_eq!(s, native);
    /// ```
    unsafe fn into_native(self) -> U;
}

impl<T, U> IntoNative<U> for *mut T
where
    U: FromForeign<Foreign = T>,
{
    unsafe fn into_native(self) -> U {
        U::from_foreign(self)
    }
}

/// A type which can be constructed from a canonical representation as a
/// C datum.
pub trait FromForeign: FreeForeign + Sized {
    /// Convert a C datum to a native Rust object, copying everything
    /// pointed to by `p` (same as `from_glib_none` in `glib-rs`)
    ///
    /// # Safety
    ///
    /// `p` must point to valid data, or can be `NULL` is `Self` is an
    /// `Option` type.
    ///
    /// ```
    /// # use foreign::FromForeign;
    /// let p = c"Hello, world!".as_ptr();
    /// let s = unsafe {
    ///     String::cloned_from_foreign(p as *const std::ffi::c_char)
    /// };
    /// assert_eq!(s, "Hello, world!");
    /// ```
    unsafe fn cloned_from_foreign(p: *const Self::Foreign) -> Self;

    /// Convert a C datum to a native Rust object, taking ownership of
    /// the pointer or Rust object (same as `from_glib_full` in `glib-rs`)
    ///
    /// The default implementation calls `cloned_from_foreign` and frees `p`.
    ///
    /// # Safety
    ///
    /// `p` must point to valid data, or can be `NULL` is `Self` is an
    /// `Option` type.  `p` becomes invalid after the function returns.
    ///
    /// ```
    /// # use foreign::{CloneToForeign, FromForeign};
    /// let s = "Hello, world!";
    /// let foreign = s.clone_to_foreign();
    /// unsafe {
    ///     assert_eq!(String::from_foreign(foreign.into_inner()), s);
    /// }
    /// // foreign is not leaked
    /// ```
    unsafe fn from_foreign(p: *mut Self::Foreign) -> Self {
        let result = Self::cloned_from_foreign(p);
        Self::free_foreign(p);
        result
    }
}

/// A RAII pointer that is automatically freed when it goes out of scope.
pub struct OwnedPointer<T: FreeForeign + ?Sized> {
    ptr: *mut <T as FreeForeign>::Foreign,
}

impl<T: FreeForeign + ?Sized> OwnedPointer<T> {
    /// Return a new `OwnedPointer` that wraps the NULL pointer.
    pub fn null_mut() -> Self {
        OwnedPointer {
            ptr: ptr::null_mut(),
        }
    }

    /// Return a new `OwnedPointer` that wraps the pointer `ptr`.
    ///
    /// # Safety
    ///
    /// The pointer must be valid and live until the returned `OwnedPointer`
    /// is dropped.
    pub unsafe fn new(ptr: *mut <T as FreeForeign>::Foreign) -> Self {
        OwnedPointer { ptr }
    }

    /// Safely create an `OwnedPointer` from one that has the same
    /// freeing function.
    /// ```
    /// # use foreign::{CloneToForeign, OwnedPointer};
    /// let s = "Hello, world!";
    /// let foreign_str = s.clone_to_foreign();
    /// let foreign_string = OwnedPointer::<String>::from(foreign_str);
    /// # assert_eq!(foreign_string.into_native(), s);
    pub fn from<U>(x: OwnedPointer<U>) -> Self
    where
        U: FreeForeign<Foreign = <T as FreeForeign>::Foreign> + ?Sized,
    {
        unsafe {
            // SAFETY: the pointer type and free function are the same,
            // only the type changes
            OwnedPointer::new(x.into_inner())
        }
    }

    /// Safely convert an `OwnedPointer` into one that has the same
    /// freeing function.
    /// ```
    /// # use foreign::{CloneToForeign, OwnedPointer};
    /// let s = "Hello, world!";
    /// let foreign_str = s.clone_to_foreign();
    /// let foreign_string: OwnedPointer<String> = foreign_str.into();
    /// # assert_eq!(foreign_string.into_native(), s);
    pub fn into<U>(self) -> OwnedPointer<U>
    where
        U: FreeForeign<Foreign = <T as FreeForeign>::Foreign>,
    {
        OwnedPointer::from(self)
    }

    /// Return the pointer that is stored in the `OwnedPointer`.  The
    /// pointer is valid for as long as the `OwnedPointer` itself.
    ///
    /// ```
    /// # use foreign::CloneToForeign;
    /// let s = "Hello, world!";
    /// let foreign = s.clone_to_foreign();
    /// let p = foreign.as_ptr();
    /// let len = unsafe { libc::strlen(p) };
    /// drop(foreign);
    /// # assert_eq!(len, 13);
    /// ```
    pub fn as_ptr(&self) -> *const <T as FreeForeign>::Foreign {
        self.ptr
    }

    pub fn as_mut_ptr(&self) -> *mut <T as FreeForeign>::Foreign {
        self.ptr
    }

    /// Return the pointer that is stored in the `OwnedPointer`,
    /// consuming the `OwnedPointer` but not freeing the pointer.
    ///
    /// ```
    /// # use foreign::CloneToForeign;
    /// let s = "Hello, world!";
    /// let p = s.clone_to_foreign().into_inner();
    /// let len = unsafe { libc::strlen(p) };
    /// // p needs to be freed manually
    /// # assert_eq!(len, 13);
    /// # unsafe { libc::free(p as *mut libc::c_void); }
    /// ```
    #[must_use]
    pub fn into_inner(mut self) -> *mut <T as FreeForeign>::Foreign {
        let result = mem::replace(&mut self.ptr, ptr::null_mut());
        mem::forget(self);
        result
    }
}

impl<T: FromForeign> OwnedPointer<T> {
    /// Convert a C datum to a native Rust object, taking ownership of
    /// the pointer or Rust object (same as `from_glib_full` in `glib-rs`)
    ///
    /// ```
    /// # use foreign::{CloneToForeign, IntoNative};
    /// let s = "Hello, world!".to_string();
    /// let foreign = s.clone_to_foreign();
    /// let native: String = unsafe {
    ///     foreign.into_native()
    ///     // foreign is not leaked
    /// };
    /// assert_eq!(s, native);
    /// ```
    pub fn into_native(self) -> T {
        // SAFETY: the pointer was passed to the unsafe constructor OwnedPointer::new
        unsafe { T::from_foreign(self.into_inner()) }
    }
}

impl<T: FreeForeign + ?Sized> Debug for OwnedPointer<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = std::any::type_name::<T>();
        let name = format!("OwnedPointer<{name}>");
        f.debug_tuple(&name).field(&self.as_ptr()).finish()
    }
}

impl<T: CloneToForeign + Default> Default for OwnedPointer<T> {
    fn default() -> Self {
        <T as Default>::default().clone_to_foreign()
    }
}

impl<T: FreeForeign + ?Sized> Drop for OwnedPointer<T> {
    fn drop(&mut self) {
        let p = mem::replace(&mut self.ptr, ptr::null_mut());
        // SAFETY: the pointer was passed to the unsafe constructor OwnedPointer::new
        unsafe { T::free_foreign(p) }
    }
}

/// A pointer whose contents were borrowed from a Rust object, and
/// therefore whose lifetime is limited to the lifetime of the
/// underlying Rust object.  The Rust object was borrowed from a
/// shared reference, and therefore the pointer is not mutable.
pub struct BorrowedPointer<P: FreeForeign + ?Sized, T> {
    ptr: *const <P as FreeForeign>::Foreign,
    storage: T,
}

impl<P: FreeForeign + ?Sized, T> BorrowedPointer<P, T> {
    /// Return a new `BorrowedPointer` that wraps the pointer `ptr`.
    /// `storage` can contain any other data that `ptr` points to,
    /// and that should be dropped when the `BorrowedPointer` goes out
    /// of scope.
    ///
    /// # Safety
    ///
    /// The pointer must be valid for the lifetime of the `BorrowedPointer`.
    /// If the pointer points into the storage, `T` must be pinned.
    pub unsafe fn new(ptr: *const <P as FreeForeign>::Foreign, storage: T) -> Self {
        BorrowedPointer { ptr, storage }
    }

    /// Return the pointer that is stored in the `BorrowedPointer`.  The
    /// pointer is valid for as long as the `BorrowedPointer` itself.
    ///
    /// ```
    /// # use foreign::BorrowForeign;
    /// # use std::ffi::CString;
    /// let s = CString::new("Hello, world!").unwrap();
    /// let borrowed = s.borrow_foreign();
    /// let len = unsafe { libc::strlen(borrowed.as_ptr()) };
    /// # assert_eq!(len, 13);
    /// ```
    pub fn as_ptr(&self) -> *const <P as FreeForeign>::Foreign {
        self.ptr
    }

    /// Safely convert a `BorrowedPointer` into one that has the same
    /// underlying type.
    ///
    /// (Not yet sure this is useful as part of the public API).
    pub(crate) fn into<Q>(self) -> BorrowedPointer<Q, T>
    where
        Q: FreeForeign<Foreign = <P as FreeForeign>::Foreign>,
    {
        BorrowedPointer {
            ptr: self.ptr,
            storage: self.storage,
        }
    }

    pub(crate) fn map<Q: FreeForeign<Foreign = P::Foreign>, F: FnOnce(T) -> U, U>(
        self,
        f: F,
    ) -> BorrowedPointer<Q, U> {
        BorrowedPointer {
            ptr: self.ptr,
            storage: f(self.storage),
        }
    }
}

impl<T: CloneToForeign + ?Sized> BorrowedPointer<T, &T> {
    /// Clone the underlying data for the receiver, creating a new
    /// C datum that contains the same data.
    ///
    /// ```
    /// # use foreign::BorrowForeign;
    /// # use std::ffi::CString;
    /// let s = CString::new("Hello, world!").unwrap();
    /// assert_eq!(s.borrow_foreign().as_ptr(), s.as_ptr());
    /// assert_ne!(s.borrow_foreign().to_owned().as_ptr(), s.as_ptr());
    /// ```
    ///
    /// ```
    /// # use foreign::BorrowForeign;
    /// # use std::ffi::CString;
    /// let s: i8 = 42;
    /// assert_eq!(s.borrow_foreign().as_ptr(), &s as *const _);
    /// assert_ne!(s.borrow_foreign().to_owned().as_ptr(), &s as *const _);
    /// ```
    pub fn to_owned(&self) -> OwnedPointer<T> {
        self.storage.clone_to_foreign()
    }
}

impl<P: FreeForeign + ?Sized, T> Debug for BorrowedPointer<P, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ptr_name = std::any::type_name::<*mut <P as FreeForeign>::Foreign>();
        let storage_name = std::any::type_name::<T>();
        let name = format!("BorrowedPointer<{ptr_name}, {storage_name}>");
        f.debug_tuple(&name).field(&self.as_ptr()).finish()
    }
}

/// A type for which a C representation can be borrowed without cloning.
pub trait BorrowForeign<'a>: FreeForeign {
    /// The type of any extra data that are needed while the `BorrowedPointer` is alive.
    type Storage: 'a;

    /// Return a wrapper for a C representation of `self`.  The wrapper
    /// borrows the data from `self` and allows access via a constant pointer.
    ///
    /// ```
    /// # use foreign::BorrowForeign;
    /// # use std::ffi::CString;
    /// let s = CString::new("Hello, world!").unwrap();
    /// let borrowed = s.borrow_foreign();
    /// let len = unsafe { libc::strlen(borrowed.as_ptr()) };
    /// # assert_eq!(len, 13);
    /// ```
    fn borrow_foreign(&'a self) -> BorrowedPointer<Self, Self::Storage>;
}

/// A type for which a C representation can be created by consuming the
/// Rust representation, hopefully without cloning much of the internal data.
pub trait IntoForeign: FreeForeign {
    /// The type of any extra data that are needed while the `BorrowedMutPointer` is alive.
    ///
    /// Usually `Self`, though does not have to be.  For example,
    /// [into_foreign()](IntoForeign::into_foreign()) could discard the
    /// unnecessary parts of `self` or perform other conversions.  In
    /// particular, a `Cow<'_, str>` will use a `CString` as the storage.
    type Storage: 'static;

    /// Return a wrapper for a C representation of `self`.  The wrapper
    /// becomes the owner of `self` and allows access via a constant pointer.
    ///
    /// ```
    /// # use foreign::IntoForeign;
    /// let s = "Hello, world!".to_string();
    /// let borrowed = s.into_foreign();
    /// let len = unsafe { libc::strlen(borrowed.as_ptr()) };
    /// # assert_eq!(len, 13);
    /// ```
    fn into_foreign(self) -> BorrowedMutPointer<Self, Self::Storage>;
}

/// A pointer whose contents were borrowed from a Rust object, and
/// therefore whose lifetime is limited to the lifetime of the
/// underlying Rust object.  The Rust object is borrowed from an
/// exclusive reference, and therefore the pointer is mutable.
pub struct BorrowedMutPointer<P: FreeForeign + ?Sized, T> {
    ptr: *mut <P as FreeForeign>::Foreign,
    storage: T,
}

impl<P: FreeForeign + ?Sized, T> BorrowedMutPointer<P, T> {
    /// Return a new `BorrowedMutPointer` that wraps the pointer `ptr`.
    /// `storage` can contain any other data that `ptr` points to,
    /// and that should be dropped when the `BorrowedMutPointer` goes out
    /// of scope.
    ///
    /// # Safety
    ///
    /// The pointer must be valid for the lifetime of the `BorrowedPointer`.
    /// If the pointer points into the storage, `T` must be pinned.
    pub unsafe fn new(ptr: *mut <P as FreeForeign>::Foreign, storage: T) -> Self {
        BorrowedMutPointer { ptr, storage }
    }

    /// Return the pointer that is stored in the `BorrowedMutPointer`.  The
    /// returned pointer is constant and is valid for as long as the
    /// `BorrowedMutPointer` itself.
    pub fn as_ptr(&self) -> *const <P as FreeForeign>::Foreign {
        self.ptr
    }

    /// Return the pointer that is stored in the `BorrowedMutPointer`.  The
    /// returned pointer is mutable and is valid for as long as the
    /// `BorrowedMutPointer` itself.
    pub fn as_mut_ptr(&mut self) -> *mut <P as FreeForeign>::Foreign {
        self.ptr
    }

    /// Safely convert a `BorrowedMutPointer` into one that has the same
    /// underlying type.
    ///
    /// (Not yet sure this is useful as part of the public API).
    pub(crate) fn into<Q>(self) -> BorrowedMutPointer<Q, T>
    where
        Q: FreeForeign<Foreign = <P as FreeForeign>::Foreign>,
    {
        BorrowedMutPointer {
            ptr: self.ptr,
            storage: self.storage,
        }
    }

    pub(crate) fn map<Q: FreeForeign<Foreign = P::Foreign>, U, F: FnOnce(T) -> U>(
        self,
        f: F,
    ) -> BorrowedMutPointer<Q, U> {
        BorrowedMutPointer {
            ptr: self.ptr,
            storage: f(self.storage),
        }
    }
}

impl<P: FreeForeign + ?Sized, T> Debug for BorrowedMutPointer<P, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = std::any::type_name::<*mut <P as FreeForeign>::Foreign>();
        let name = format!("BorrowedMutPointer<{name}>");
        f.debug_tuple(&name).field(&self.as_ptr()).finish()
    }
}

/// A type for which a C representation can be borrowed mutably without cloning.
pub trait BorrowForeignMut<'a>: FreeForeign {
    /// The type of any extra data that are needed while the `BorrowedMutPointer` is alive.
    type Storage: 'a;

    /// Return a wrapper for a C representation of `self`.  The wrapper
    /// borrows the data from `self` and allows access via a mutable pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// # use foreign::BorrowForeignMut;
    /// let mut i = 123i8;
    /// let mut borrowed = i.borrow_foreign_mut();
    /// unsafe {
    ///     assert_eq!(*borrowed.as_ptr(), 123i8);
    ///     *borrowed.as_mut_ptr() = 45i8;
    /// }
    /// assert_eq!(i, 45i8);
    /// ```
    /// is analogous to:
    /// ```
    /// let mut i = 123i8;
    /// let borrowed = &mut i;
    /// assert_eq!(*borrowed, 123i8);
    /// *borrowed = 45i8;
    /// assert_eq!(i, 45i8);
    /// ```
    ///
    /// For integer types, or anything that implements [`FixedAlloc`], it is also possible
    /// to borrow from a `Vec` or array:
    ///
    /// ```
    /// # use std::ffi::{CStr, c_char};
    /// # use foreign::BorrowForeignMut;
    /// let mut v: [u8; 64] = [0; 64];
    /// # if cfg!(miri) { return; }
    /// unsafe {
    ///     libc::sprintf(v.borrow_foreign_mut().as_mut_ptr().cast::<c_char>(),
    ///                   c"hello %s".as_ptr(),
    ///                   c"world".as_ptr());
    /// }
    /// assert_eq!(CStr::from_bytes_until_nul(&v).unwrap(), c"hello world");
    /// ```
    fn borrow_foreign_mut(&'a mut self) -> BorrowedMutPointer<Self, Self::Storage>;
}
