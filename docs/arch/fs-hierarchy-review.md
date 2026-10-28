# Revue Architecturale : Filesystem Hierarchy

## Date : 2026-10-13
## Contexte : PR #24 (feature/fs-hierarchy)

## 1. État actuel

### Ce qui est bien (mécanisme pur)
- `src/fat32.rs` : driver FAT32 avec `create_dir`, résolution de chemins. C'est du mécanisme — le kernel sait lire/écrire des chemins, sans savoir ce qu'ils signifient. ✅
- `SYS_MKDIR` (12), `SYS_READ_FILE`, `SYS_WRITE_FILE`, `SYS_LS` : prennent des chemins, les passent au driver. ✅
- `tools/mkdisk.sh` : crée la structure au build. C'est l'"installateur". ✅

### Ce qui viole nos principes (politique dans le kernel)
- **`sys_spawn` résout `/bin/<nom>`** : Le kernel décide où chercher les programmes. C'est une politique qui appartient au shell.

## 2. Principes applicables

### Mécanisme vs Politique (notre principe fondamental)
- **Kernel** : `spawn(path)` — prend un chemin exact, charge le binaire. Ne sait pas ce que `/bin` veut dire.
- **Shell (forge)** : maintient un `PATH`, résout `snake` → `/bin/SNAKE.BIN`. C'est lui qui connaît la convention.

### FHS (Filesystem Hierarchy Standard, Linux)
```
/bin/   → binaires essentiels (notre cas : tous les programmes user)
/etc/   → configuration système
/home/  → répertoires utilisateurs
/tmp/   → fichiers temporaires
/dev/   → devices (filesystem virtuel, futur)
/proc/  → infos processus (filesystem virtuel, futur)
```

### "Tout est un fichier" (Unix)
- `/dev/mouse` : lire = position souris, écrire = configurer
- `/proc/42/cmdline` : infos sur le processus 42
- Ces sont des filesystems virtuels — le kernel les génère, pas le disque.

## 3. Corrections requises

### 3.1. Retirer la politique de `sys_spawn`
**Avant** (actuel) :
```rust
// Kernel décide : essaie /bin/<nom>, puis <nom>
if !path.contains('/') {
    try /bin/<path>, else try <path>
}
```

**Après** :
```rust
// Kernel : prend le chemin tel quel, point.
load_program_from_disk(path)
```

### 3.2. Ajouter PATH dans forge
Le shell maintient une liste de recherche :
```
PATH = ["/bin/", "./"]
```
Quand l'utilisateur tape `snake` :
1. Si contient `/` → utiliser tel quel
2. Sinon → essayer `/bin/SNAKE.BIN`, puis `SNAKE.BIN` (cwd, quand on aura un cwd)

### 3.3. Mettre à jour les appelants
- **WM** : utilise déjà `/bin/PORTAL.BIN` (chemin complet) ✅ — pas de changement
- **forge** : la commande `./snake` devient redondante ; `snake` seul suffit avec PATH
- **Tests** : mettre à jour

## 4. Vision future

### Court terme
- `forge` : variable `PATH`, commande `cd` (cwd par processus)
- `/dev/` : commencer par `/dev/null`, `/dev/zero` (simples)

### Moyen terme
- `/proc/` : `pid/`, `cmdline`, `meminfo`
- Permissions Unix (rwx) — pour l'instant tout est 777
- Liens symboliques

### Long terme
- Plusieurs filesystems montés (`/mnt/`, etc.)
- `PATH` configurable via `/etc/profile`

## 5. Décisions

| Question | Décision | Raison |
|----------|----------|--------|
| Où résoudre les noms ? | Dans `forge`, pas le kernel | Séparation mécanisme/politique |
| Format du PATH ? | Liste de préfixes, `:`-séparés comme Unix | Familiarité |
| Backward compat root ? | Non — on casse proprement, on est pré-1.0 | Dette technique sinon |
| `/dev/` maintenant ? | Non — documenté comme futur | Scope contrôlé |
