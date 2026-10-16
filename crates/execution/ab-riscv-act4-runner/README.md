# ab-riscv-act4-runner

Runner for the RISC-V Architectural Certification Tests (ACTs), specifically the ACT4.

`res` directory contains the definition of the cores for building test ELFs that can then be run by the
`ab-riscv-act4-runner`.

The workflow is generally the following:

```bash
# ACT4 commit to use, there is no release with Sail 0.15 yet
ACT4_COMMIT=d3b1032a5fd553cc51a830bd771e76fc22358a22
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
# Takes a few minutes, if you plan to call it repeatedly then remove `make` and call it from within instead.
# `apt-get` and `mise deps` are only needed because the image doesn't ship UDB gems pre-installed right now.
# `ZawrsSm` is excluded because its tests wait for interrupts that these cores don't implement.
docker run -it --rm \
    -u 0:0 \
    -v ./riscv-arch-test:/act4 \
    -v ./work:/act4/work \
    -v ./res/abundance:/act4/config/abundance:ro \
    -e CONFIG_FILES="${CONFIG_FILES[*]}" \
    -e EXCLUDE_EXTENSIONS="ZawrsSm" \
    -e HOST_USER="$(id -u):$(id -g)" \
    "ghcr.io/riscv/act4-build:sha-${ACT4_COMMIT}" \
    sh -c '
        apt-get update &&
        apt-get install -y --no-install-recommends build-essential &&
        mise deps --skip prek &&
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
