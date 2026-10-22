custom_target('blkio.3',
  command : ['rst2man', '@INPUT@', '@OUTPUT@'],
  input : ['blkio.rst'],
  output : ['blkio.3'],
  install : true,
  install_dir : join_paths(get_option('mandir'), 'man3'))
