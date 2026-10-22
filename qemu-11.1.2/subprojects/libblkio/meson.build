project('libblkio', ['c', 'rust'],
  meson_version: '>=0.61.0',
  license : 'MIT OR Apache-2.0',
  version : run_command(['./package-version.py'], check : true).stdout().strip(),
  default_options : ['buildtype=release'],
)

add_project_arguments(
    '-D_GNU_SOURCE',
    language : 'c',
)

subdir('docs')
subdir('include')
subdir('src') # requires: include

libblkio_dep = declare_dependency(include_directories: inc,
                                  link_with: lib)

subdir('examples') # requires: include, src
subdir('tests') # requires: include, src
