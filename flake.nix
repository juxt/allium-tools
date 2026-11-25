{
  description = "Allium CLI and language server";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          lib = pkgs.lib;
          version = (lib.importTOML ./Cargo.toml).workspace.package.version;

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./crates
              ./package.json
              ./package-lock.json
              ./packages
              ./extensions/allium/package.json
              ./extensions/allium/src
              # Read by the CLI's property tests, which run during the build.
              ./docs/project/specs
              ./docs/allium-v3-language-reference.md
            ];
          };

          # wasm-bindgen-cli must match the wasm-bindgen crate exactly.
          wasmBindgenVersion =
            (lib.findFirst (p: p.name == "wasm-bindgen") null (lib.importTOML ./Cargo.lock).package).version;
          wasmBindgenCli =
            pkgs."wasm-bindgen-cli_${lib.replaceStrings [ "." ] [ "_" ] wasmBindgenVersion}"
              or (throw "nixpkgs has no wasm-bindgen-cli ${wasmBindgenVersion}; update the nixpkgs input");

          allium = pkgs.rustPlatform.buildRustPackage {
            pname = "allium";
            inherit version src;
            cargoLock.lockFile = ./Cargo.lock;
            cargoBuildFlags = [ "-p" "allium-cli" ];
            cargoTestFlags = [ "-p" "allium-cli" ];
            meta = {
              description = "CLI for checking Allium specification files";
              homepage = "https://github.com/juxt/allium-tools";
              license = lib.licenses.mit;
              mainProgram = "allium";
            };
          };

          allium-lsp = pkgs.stdenv.mkDerivation {
            pname = "allium-lsp";
            inherit version src;

            cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
            npmDeps = pkgs.importNpmLock { npmRoot = src; };
            npmConfigHook = pkgs.importNpmLock.npmConfigHook;
            # Only the language server's bundle is built; the extension's
            # native dependencies (keytar, via vsce) are never needed.
            npmRebuildFlags = [ "--ignore-scripts" ];

            nativeBuildInputs = [
              pkgs.rustPlatform.cargoSetupHook
              pkgs.cargo
              pkgs.rustc
              pkgs.lld
              wasmBindgenCli
              pkgs.nodejs
              pkgs.importNpmLock.npmConfigHook
              pkgs.makeWrapper
            ];

            # The wasm parser is a workspace package that wasm-bindgen generates.
            postPatch = ''
              mkdir -p packages/allium-parser-wasm
              echo '{"name":"allium-parser-wasm","version":"${version}","main":"allium_wasm.js"}' \
                > packages/allium-parser-wasm/package.json
            '';

            buildPhase = ''
              runHook preBuild
              cargo build --release --offline -p allium-wasm --target wasm32-unknown-unknown
              wasm-bindgen --target nodejs --out-dir packages/allium-parser-wasm \
                target/wasm32-unknown-unknown/release/allium_wasm.wasm
              npm run --workspace packages/allium-lsp build
              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              mkdir -p $out/lib/allium-lsp
              cp packages/allium-lsp/dist/{bin.js,allium_wasm.js,allium_wasm_bg.wasm} $out/lib/allium-lsp/
              makeWrapper ${lib.getExe pkgs.nodejs} $out/bin/allium-lsp \
                --add-flags $out/lib/allium-lsp/bin.js
              runHook postInstall
            '';

            meta = {
              description = "Language server for Allium";
              homepage = "https://github.com/juxt/allium-tools";
              license = lib.licenses.mit;
              mainProgram = "allium-lsp";
            };
          };
        in
        {
          inherit allium allium-lsp;
          default = pkgs.symlinkJoin {
            name = "allium-tools-${version}";
            paths = [
              allium
              allium-lsp
            ];
          };
        }
      );

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.lld
            pkgs.wasm-pack
            pkgs.nodejs
          ];
        };
      });
    };
}
