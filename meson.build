project('local-ai-workstation', 'c', version: '0.1.0', meson_version: '>= 1.3.0')

gnome = import('gnome')
python = import('python').find_installation('python3')
if not python.language_version().version_compare('>= 3.11')
  error('Repository checks need Python 3.11 or newer')
endif
cargo = find_program('cargo')
dependency('gtk4', version: '>= 4.12')
dependency('libadwaita-1', version: '>= 1.4')

profile = get_option('buildtype') == 'release' ? 'release' : 'debug'
custom_target(
  'local-ai-workstation',
  output: 'local-ai-workstation',
  command: [
    python, files('scripts/build_app.py'), cargo.full_path(),
    meson.project_source_root(), meson.project_build_root() / 'cargo-target',
    '@OUTPUT@', profile,
  ],
  build_always_stale: true,
  build_by_default: true,
  install: true,
  install_dir: get_option('bindir'),
  install_mode: 'rwxr-xr-x',
)

app_id = 'org.gnome.LocalAIWorkstation'
desktop_file = configure_file(
  input: 'data' / app_id + '.desktop.in',
  output: app_id + '.desktop',
  copy: true,
  install: true,
  install_dir: get_option('datadir') / 'applications',
)
metainfo_file = configure_file(
  input: 'data' / app_id + '.metainfo.xml.in',
  output: app_id + '.metainfo.xml',
  copy: true,
  install: true,
  install_dir: get_option('datadir') / 'metainfo',
)
install_data(
  'data' / app_id + '.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0/schemas',
)
install_data(
  'data/icons/hicolor/scalable/apps' / app_id + '.svg',
  install_dir: get_option('datadir') / 'icons/hicolor/scalable/apps',
)
gnome.post_install(
  glib_compile_schemas: true,
  gtk_update_icon_cache: true,
  update_desktop_database: true,
)

test('repository', python, args: [files('scripts/check_repository.py')])
test('python-checks', python, args: ['-m', 'unittest', 'discover', '-s', 'tests', '-p', 'test_*.py'],
  workdir: meson.project_source_root())
test('desktop', find_program('desktop-file-validate'), args: [desktop_file])
# The unpublished scaffold has no homepage yet. Keep all other validation at its normal severity.
test('appstream', find_program('appstreamcli'),
  args: ['validate', '--no-net', '--override=url-homepage-missing=info', metainfo_file])
test('settings', find_program('glib-compile-schemas'),
  args: ['--strict', '--dry-run', meson.project_source_root() / 'data'])
