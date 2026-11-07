# Source me: environment for running the frozen Python answer key without writing into it.
# Usage: FBV2_DB=fbv2_l1_xxx . crates/fb-harness/py/env.sh
export FB_REFERENCE="${FB_REFERENCE:-/mnt/Data/Projects/FreedomBooks/wt/reference}"
export FB_PY="${FB_PY:-/mnt/Data/Projects/FreedomBooks/wt/f2/.venv/bin/python}"
export FB_HARNESS_VAR="${FB_HARNESS_VAR:-$HOME/.hermes/profiles/freedombooks/cache/scratch/fbv2-l1-var}"
mkdir -p "$FB_HARNESS_VAR/attachments" "$FB_HARNESS_VAR/backups" "$FB_HARNESS_VAR/tmp"
export FB_ENV_FILE=""
export FB_DEBUG=1
export PYTHONDONTWRITEBYTECODE=1
export FB_DEV_SECRET_KEY_FILE="$FB_HARNESS_VAR/dev-secret-key"
export FB_ATTACHMENTS_DIR="$FB_HARNESS_VAR/attachments"
export FB_BACKUP_DIR="$FB_HARNESS_VAR/backups"
export FB_BACKUP_TEMP_DIR="$FB_HARNESS_VAR/tmp"
export FB_DB_NAME="${FBV2_DB:?set FBV2_DB to an approved fbv2 harness database name}"
case "$FB_DB_NAME" in fbv2_l4b_*|fbv2_l4t_*|fbv2_l4a_*|fbv2_l4r_*|fbv2_l1_*|fbv2_l2a_*|fbv2_l3s_*|fbv2_l3p_*|fbv2_l3c_*|fbv2_l3d_*|fbv2_l3r_*|fbv2_l3b_*|fbv2_int*|fbv2_l4f_*|fbv2_core_*) ;; *) echo "refusing DB name $FB_DB_NAME" >&2; return 1 2>/dev/null || exit 1;; esac
export DJANGO_SETTINGS_MODULE=config.settings
export PYTHONPATH="$FB_REFERENCE"
