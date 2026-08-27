#!/bin/zsh

set -eu

app_path="/Applications/GitBoost.app"

echo "GitBoost 安装助手"
echo

if [[ ! -d "$app_path" || -L "$app_path" ]]; then
  echo "未找到 $app_path"
  echo "请先把 GitBoost 拖入“应用程序”文件夹，再重新运行此助手。"
  echo
  read -r "?按回车键退出..."
  exit 1
fi

bundle_identifier=$(
  /usr/libexec/PlistBuddy -c "Print :CFBundleIdentifier" \
    "$app_path/Contents/Info.plist" 2>/dev/null || true
)
if [[ "$bundle_identifier" != "pro.gitboost.desktop" ]]; then
  echo "$app_path 不是预期的 GitBoost 应用，未执行任何修改。"
  echo
  read -r "?按回车键退出..."
  exit 1
fi

echo "即将移除 GitBoost 的 macOS 隔离标记。"
echo "请输入当前 Mac 管理员账户的密码；输入时终端不会显示字符。"
echo

/usr/bin/sudo /usr/bin/xattr -rd com.apple.quarantine "$app_path"

echo
echo "处理完成，现在可以从“应用程序”中打开 GitBoost。"
read -r "?按回车键退出..."
