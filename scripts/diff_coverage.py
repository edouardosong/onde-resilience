#!/usr/bin/env python3
"""Gate diff-coverage ONDE (T26) — stdlib uniquement.

Entrées :
  * rapport JSON exporté par `cargo llvm-cov --workspace --locked --json`
    (format officiel llvm.coverage.json.export),
  * liste des fichiers .rs modifiés vs la base de diff (un chemin par ligne,
    relatif à la racine du dépôt, ex. `core/src/network/mod.rs`).

Métrique : couverture LIGNES agrégée UNIQUEMENT sur les fichiers modifiés
(non supprimés) du workspace `core/`, pondérée par nombre de lignes.
Les fichiers hors `core/` et les exclusions déclarées sont ignorés.

Exclusions par défaut (décision utilisateur 2026-08-23, cf.
reports/t22-coverage/proposition-gate.md) :
  * `src/bin/*`                — binaires CLI/glue, candidats tests e2e ;
  * `**/whisper_cpp_sys/**`    — bindings C vendored (déjà hors workspace) ;
  * `**/build.rs`              — script de build, jamais instrumenté ;
  * `**/target/**`             — artefacts de compilation.

Notes : les blocs `#[cfg(test)]` ne s'exécutent que sous tests (donc couverts
par construction) ; le code d'un test `#[ignore]` n'est PAS exécuté, il
apparaît donc comme non couvert — comportement voulu (T22 §4).

Codes de sortie :
  0  PASS (>= seuil), ou bande de tolérance [seuil-tolérance, seuil[ (WARN),
     ou « nothing to measure » (aucun fichier mesurable après exclusions /
     push sans base de diff) — jamais de faux rouge ;
  1  FAIL : agrégat < seuil - tolérance ;
  2  erreur de configuration / entrée illisible.

Variables d'environnement : SEUIL (seuil %), TOLERANCE (points).
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import sys
from pathlib import PurePosixPath

# ── Exclusions déclarées (visibles, pas cachées — T22 §4) ──────────────────
DEFAULT_EXCLUDES = [
    "src/bin/*",
    "**/whisper_cpp_sys/**",
    "**/build.rs",
    "**/target/**",
]

WORKSPACE_DIR = "core"  # périmètre mesuré par `cargo llvm-cov --workspace`


def _norm_repo_path(raw: str) -> str | None:
    """Normalise un chemin en forme dépôt-relative POSIX `core/...`.

    Retourne None si le chemin n'appartient pas au workspace core/.
    """
    p = PurePosixPath(raw.replace("\\", "/"))
    parts = [x for x in p.parts if x not in (".",)]
    while parts and parts[0] == "..":
        parts = parts[1:]
    # tolère un préfixe absolu machine : garde ce qui suit le dernier "core/"
    if parts and parts[0] == WORKSPACE_DIR:
        return str(PurePosixPath(*parts))
    if WORKSPACE_DIR in parts:
        idx = len(parts) - 1 - parts[::-1].index(WORKSPACE_DIR)
        return str(PurePosixPath(*parts[idx:]))
    return None


def load_report_index(cov_json_path: str) -> dict[str, dict]:
    """Indexe le rapport llvm-cov par chemin dépôt-relative core/... .

    Clé = chemin normalisé ; valeur = dict {covered, total, percent}.
    """
    with open(cov_json_path, encoding="utf-8") as fh:
        data = json.load(fh)

    index: dict[str, dict] = {}
    for block in data.get("data", []):
        for fentry in block.get("files", []):
            key = _norm_repo_path(fentry.get("filename", ""))
            if key is None:
                continue
            summary = fentry.get("summary") or {}
            lines = summary.get("lines") or {}
            covered = int(lines.get("covered", 0))
            total = int(lines.get("count", 0))
            if total == 0 and "segments" in fentry:
                covered, total = _lines_from_segments(fentry["segments"])
            index[key] = {"covered": covered, "total": total}
    return index


def _lines_from_segments(segments: list) -> tuple[int, int]:
    """Repli sans `summary` : compte les lignes depuis les segments LLVM."""
    per_line: dict[int, int] = {}
    for seg in segments:
        try:
            line, _col, count, has_count = int(seg[0]), int(seg[1]), int(seg[2]), bool(seg[3])
        except (ValueError, IndexError, TypeError):
            continue
        if not has_count:
            continue
        prev = per_line.get(line, 0)
        per_line[line] = max(prev, count)
    covered = sum(1 for c in per_line.values() if c > 0)
    return covered, len(per_line)


def _core_relative(rel: str) -> str:
    """Retire le préfixe 'core/' pour matcher les motifs du périmètre workspace."""
    prefix = WORKSPACE_DIR + "/"
    return rel[len(prefix):] if rel.startswith(prefix) else rel


def parse_args(argv: list[str]) -> argparse.Namespace:
    ap = argparse.ArgumentParser(
        description="Gate diff-coverage lignes sur fichiers .rs modifiés (workspace core/)."
    )
    ap.add_argument("--cov", required=True, help="chemin du rapport JSON cargo-llvm-cov")
    ap.add_argument(
        "--modified-file",
        action="append",
        default=[],
        help="fichier liste des chemins modifiés (un/ligne, '-' = stdin) ; répétable",
    )
    ap.add_argument("files", nargs="*", help="chemins modifiés passés directement")
    ap.add_argument("--threshold", type=float,
                    default=float(os.environ.get("SEUIL", 80)),
                    help="seuil %% lignes (défaut env SEUIL=80)")
    ap.add_argument("--tolerance", type=float,
                    default=float(os.environ.get("TOLERANCE", 2)),
                    help="marge ± points (défaut env TOLERANCE=2)")
    ap.add_argument("--exclude", action="append", default=[],
                    help="glob supplémentaire exclu (répétable)")
    ap.add_argument("--label", default="", help="contexte affiché (ex. base de diff)")
    return ap.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    threshold, tolerance = args.threshold, args.tolerance
    excludes = DEFAULT_EXCLUDES + list(args.exclude)

    def _pattern_variants(pat: str) -> tuple[str, ...]:
        # '**/x' doit matcher 'x' à la racine comme en profondeur.
        if pat.startswith("**/"):
            return (pat, pat[len("**/"):])
        return (pat,)

    def excluded(rel: str) -> bool:
        candidates = (rel, _core_relative(rel))
        return any(
            fnmatch.fnmatch(cand, pv)
            for pat in excludes
            for pv in _pattern_variants(pat)
            for cand in candidates
        )

    # ── lecture de la liste des fichiers modifiés ────────────────────────
    raw_modified: list[str] = list(args.files)
    for list_path in args.modified_file:
        if list_path == "-":
            raw_modified.extend(l.strip() for l in sys.stdin if l.strip())
        else:
            with open(list_path, encoding="utf-8") as fh:
                raw_modified.extend(l.strip() for l in fh if l.strip())

    out_scope: list[str] = []       # hors workspace core/
    excl: list[str] = []            # exclusions déclarées
    wanted: list[str] = []          # à mesurer
    seen: set[str] = set()
    for raw in raw_modified:
        rel = _norm_repo_path(raw)
        if rel is None:
            out_scope.append(raw)
            continue
        if rel in seen:
            continue
        seen.add(rel)
        if excluded(rel):
            excl.append(rel)
            continue
        wanted.append(rel)

    banner = f"[diff-coverage] seuil={threshold:g}% tolérance=±{tolerance:g} pts"
    if args.label:
        banner += f" base={args.label}"
    print(banner)

    if not wanted:
        print("*** PASS — nothing to measure ***")
        print(f"  .rs modifiés hors périmètre core/ : {len(out_scope)}")
        print(f"  exclusions déclarées appliquées   : {len(excl)}")
        for p in excl:
            print(f"    exclu : {p}")
        return 0

    # ── index du rapport de couverture ───────────────────────────────────
    try:
        index = load_report_index(args.cov)
    except FileNotFoundError:
        print(f"ERREUR CONFIG : rapport introuvable : {args.cov}", file=sys.stderr)
        return 2
    except json.JSONDecodeError as exc:
        print(f"ERREUR CONFIG : JSON invalide ({args.cov}) : {exc}", file=sys.stderr)
        return 2

    rows, not_instrumented = [], []
    tot_cov = tot_all = 0
    for rel in wanted:
        info = index.get(rel)
        if info is None or info["total"] == 0:
            # Pas instrumenté (non compilé dans une cible testée) : signalé,
            # NE fait PAS échouer la gate (jamais de faux rouge).
            not_instrumented.append(rel)
            continue
        pct = 100.0 * info["covered"] / info["total"]
        rows.append((rel, info["covered"], info["total"], pct))
        tot_cov += info["covered"]
        tot_all += info["total"]

    W = 78
    print("=" * W)
    print(f"{'STATUT':<8} {'%':>7} {'lignes':>13}  fichier (core-relatif)")
    print("-" * W)
    for rel, c, t, pct in sorted(rows, key=lambda r: r[3]):
        mark = "ok" if round(pct, 2) >= threshold else "MANQUE"
        print(f"{mark:<8} {pct:>6.2f}% {c:>5}/{t:<6}  {rel}")
    print("-" * W)

    for rel in not_instrumented:
        print(f"AVERTISSEMENT : modifié mais absent du rapport (non instrumenté) : {rel}")
    for p in out_scope:
        print(f"hors périmètre core/ (ignoré) : {p}")
    for p in excl:
        print(f"exclu (règle déclarée) : {p}")

    if tot_all == 0:
        print("*** PASS — nothing to measure *** (fichiers modifiés sans lignes instrumentables)")
        return 0

    agg = 100.0 * tot_cov / tot_all
    print(f"AGREGAT diff : {agg:.2f}% ({tot_cov}/{tot_all} lignes) "
          f"sur {len(rows)} fichier(s) modifié(s) mesuré(s)")

    if agg >= threshold:
        print(f"*** PASS — {agg:.2f}% >= seuil {threshold:g}% ***")
        return 0
    if agg >= threshold - tolerance:
        print(f"*** WARN (exit 0) — {agg:.2f}% dans la bande de tolérance "
              f"[{threshold - tolerance:g}%, {threshold:g}%[ — revue humaine conseillée ***")
        return 0
    worst = min(rows, key=lambda r: r[3]) if rows else None
    print(f"*** FAIL — {agg:.2f}% < {threshold - tolerance:g}% (seuil {threshold:g}% - tolérance "
          f"{tolerance:g} pts) ***")
    if worst:
        print(f"  pire fichier : {worst[0]} {worst[3]:.2f}% ({worst[1]}/{worst[2]} lignes)")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
