"""Loader simulation: find which imported (DLL, function) pair causes
STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) for the Rust test binary.

Resolution order mirrors Windows: exe dir -> cwd -> System32 -> PATH.
API-set stubs live in System32 on Win10/11, so parsing them directly works.
"""
import os, sys
import pefile

EXE = r"src-tauri\target\debug\deps\nexora_game_studio_lib-53b839279d28a31a.exe"
exe_dir = os.path.dirname(os.path.abspath(EXE))

search_dirs = []
def add(d):
    if d and os.path.isdir(d) and d not in search_dirs:
        search_dirs.append(d)
add(exe_dir)
add(os.path.join(exe_dir, ".."))
add(os.path.join(exe_dir, "..", ".."))
sysroot = os.environ.get("SystemRoot", r"C:\Windows")
add(os.path.join(sysroot, "System32"))
for d in os.environ.get("PATH", "").split(os.pathsep) + os.environ.get("ext", "").split(";"):
    add(d)

def find_dll(name):
    for d in search_dirs:
        c = os.path.join(d, name)
        if os.path.isfile(c):
            return c
    return None

# Load each DLL's export table once
eat_cache = {}
def exports_of(path):
    if path in eat_cache:
        return eat_cache[path]
    pe = pefile.PE(path, fast_load=True)
    pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_EXPORT']])
    exports, ords = set(), set()
    exp = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
    if exp:
        for s in exp.symbols:
            if s.name:
                exports.add(s.name.decode().lower())
            ords.add(s.ordinal)
    eat_cache[path] = (exports, ords)
    return eat_cache[path]

problems = []
checked = 0
dll_cache = {}
pe = pefile.PE(EXE, fast_load=True)
pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY['IMAGE_DIRECTORY_ENTRY_IMPORT']])
for dll_entry in pe.DIRECTORY_ENTRY_IMPORT:
    dll = dll_entry.dll.decode().lower()
    path = find_dll(dll)
    dll_cache[dll] = path
    if path is None:
        problems.append((dll, "?", "DLL FILE NOT FOUND in search order"))
        continue
    exports, ords = exports_of(path)
    for imp in dll_entry.imports:
        checked += 1
        if imp.name:
            fn = imp.name.decode().lower()
            if fn not in exports:
                problems.append((dll, fn, f"missing entry point in {path}"))
        elif imp.ordinal is not None and imp.ordinal not in ords:
            problems.append((dll, f"ordinal {imp.ordinal}", f"missing ordinal in {path}"))

print(f"checked {checked} imported functions across {len(dll_cache)} DLLs")
for d, path in sorted(dll_cache.items()):
    print(f"  {d:45s} -> {path}")
if problems:
    print("\n== PROBLEMS FOUND:")
    for dll, fn, msg in problems:
        print(f"  {dll} :: {fn} :: {msg}")
    sys.exit(1)
print("\nNO missing entry points via file-level simulation (static imports clean).")
