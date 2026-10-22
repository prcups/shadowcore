cargo_build = find_program('cargo-build.sh')

v = meson.project_version().split('.')

real_name = 'libblkio.so.@0@.@1@.@2@'.format(v[0], v[1], v[2])
soname    = 'libblkio.so.@0@'.format(v[0])

# src/cargo-build.sh generates "real name" lib, e.g. libblkio.so.1.0.0

real_name_target = custom_target(real_name,
  build_by_default : true,
  build_always_stale : true,
  command : [cargo_build, get_option('debug').to_string(),
             get_option('optimization'), meson.current_build_dir() / 'target',
             meson.project_version()],
  output : [real_name],
  install : true,
  install_dir : get_option('libdir'))

# create symlink from soname to real name, e.g. libblkio.so.1 ->
# libblkio.so.1.0.0; must have separate install command since directly installing
# the custom_target() wouldn't preserve the symlink

soname_target = custom_target(soname,
  build_by_default : true,
  build_always_stale : true,
  depends : [real_name_target],
  command : ['ln', '-fs', real_name, meson.current_build_dir() / soname],
  output : [soname])

install_symlink(
  soname,
  pointing_to: real_name,
  install_dir: get_option('libdir'))

# create symlink from "linker name" to soname, e.g. libblkio.so -> libblkio.so.1

lib = custom_target('libblkio.so',
  build_by_default : true,
  build_always_stale : true,
  depends : [soname_target],
  command : ['ln', '-fs', soname, meson.current_build_dir() / 'libblkio.so'],
  output : ['libblkio.so'])

install_symlink(
  'libblkio.so',
  pointing_to: soname,
  install_dir: get_option('libdir'))

# generate pkg-config file

import('pkgconfig').generate(libraries : ['-lblkio'],
  name : 'blkio',
  description : 'Block device I/O library')
