#!/usr/bin/env bash
# 备份 Beview 库（pg_dump custom 格式）+ 主密钥到 backup/<时间戳>/，保留最近 30 份。
#
# 用法:
#   ./scripts/backup.sh            # 从 server/config.toml 读 database_url
#   DATABASE_URL=... ./scripts/backup.sh
#
# 依赖: pg_dump（postgresql-client）。本机可能没装；可在任意有 pg 客户端、
#       能访问该 PostgreSQL 实例的机器上执行（含数据库所在主机）。
#
# ⚠️ 安全提示: 备份包含 settings 表（含 LLM api_key 密文）与 users（密码哈希）。
#    api_key 密文的解密依赖 server/.master_key —— 本脚本会把它一并拷入备份目录，
#    缺了它恢复后所有密钥不可解（评审整改：此前只备 DB 不备钥匙）。请妥善保管整个目录。
set -euo pipefail
cd "$(dirname "$0")/.."

DB_URL="${DATABASE_URL:-$(grep -oP 'database_url\s*=\s*"\K[^"]+' server/config.toml || true)}"
if [ -z "$DB_URL" ]; then
  echo "未找到 database_url，请用 DATABASE_URL 环境变量指定" >&2
  exit 1
fi

STAMP="$(date +%Y%m%d_%H%M%S)"
OUT_DIR="backup/${STAMP}"
mkdir -p "$OUT_DIR"

pg_dump "$DB_URL" --format=custom --no-owner -f "$OUT_DIR/db.dump"
echo "已备份数据库: $OUT_DIR/db.dump"

MASTER_KEY="server/.master_key"
if [ -f "$MASTER_KEY" ]; then
  cp "$MASTER_KEY" "$OUT_DIR/master_key"
  chmod 600 "$OUT_DIR/master_key" 2>/dev/null || true
  echo "已备份主密钥: $OUT_DIR/master_key（恢复时放回 server/.master_key）"
else
  echo "⚠️  未找到 $MASTER_KEY —— 若库中存有加密 api_key，此份备份将不可完整恢复" >&2
fi

echo "已备份: $OUT_DIR/"
# 仅保留最近 30 份（按目录）
ls -1dt backup/*/ 2>/dev/null | tail -n +31 | xargs -r rm -rf
echo "backup/ 保留最近 30 份。"
