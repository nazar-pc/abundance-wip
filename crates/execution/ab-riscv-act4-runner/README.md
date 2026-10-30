# ab-riscv-act4-runner

Runner for the RISC-V Architectural Certification Tests (ACTs), specifically the ACT4.

`res` directory contains the definition of the cores for building test ELFs that can then be run by the
`ab-riscv-act4-runner`.

The workflow is generally the following:

```bash
# ACT4 commit to use, there is no release with Sail 0.16 yet
ACT4_COMMIT=0643dc10d7e38ba1ba5f9acfcf5105bb5a818da0
git clone https://github.com/riscv/riscv-arch-test
git -C riscv-arch-test checkout "${ACT4_COMMIT}"
# Joined with spaces below, ACT4's Makefile doesn't handle newlines
CONFIG_FILES=(
    config/abundance/abundance-rv32i-zve32x-zvbb/test_config.yaml
    config/abundance/abundance-rv64i-zve32x-zvbb/test_config.yaml
    config/abundance/abundance-rv32i-max/test_config.yaml
    config/abundance/abundance-rv64i-max/test_config.yaml
)
mkdir -p work
# Takes a few minutes, if you plan to call it repeatedly then remove `make` and call it from within instead
docker run -it --rm \
    -u 0:0 \
    -v ./riscv-arch-test:/act4 \
    -v ./work:/act4/work \
    -v ./res/abundance:/act4/config/abundance:ro \
    -e CONFIG_FILES="${CONFIG_FILES[*]}" \
    -e HOST_USER="$(id -u):$(id -g)" \
    "ghcr.io/riscv/act4-build:sha-${ACT4_COMMIT}" \
    sh -c '
        make vector-tests &&
        make &&
        chown -R "${HOST_USER}" work
    '
# Run generated test ELFs against the interpreter, the first argument is the name of the core without `abundance-`
# prefix
cargo run -- rv32i-zve32x-zvbb work/abundance-rv32i-zve32x-zvbb/elfs
cargo run -- rv64i-zve32x-zvbb work/abundance-rv64i-zve32x-zvbb/elfs
cargo run -- rv32i-max work/abundance-rv32i-max/elfs
cargo run -- rv64i-max work/abundance-rv64i-max/elfs
```
