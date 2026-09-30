// Package settings owns the user-facing routing policy and saved preferences.
package settings

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

type Config struct {
	DeviceUID string `json:"device_uid"`
	Left      int    `json:"left"` // One-based hardware channel number.
	Right     int    `json:"right"`
	Mode      string `json:"mode"` // mono, stereo, or mix (averaged mono).
	Muted     bool   `json:"muted"`
}

func Default() Config { return Config{Left: 1, Right: 2, Mode: "mono"} }

func (c Config) Validate(channels int) error {
	if c.DeviceUID == "" {
		return fmt.Errorf("入力デバイスを選択してください")
	}
	if c.Mode != "mono" && c.Mode != "stereo" && c.Mode != "mix" {
		return fmt.Errorf("不正な出力モードです")
	}
	if c.Left < 1 || c.Left > channels {
		return fmt.Errorf("入力チャンネルが範囲外です（1〜%d）", channels)
	}
	if c.Mode != "mono" && (c.Right < 1 || c.Right > channels) {
		return fmt.Errorf("右チャンネルが範囲外です（1〜%d）", channels)
	}
	return nil
}

func Path() (string, error) {
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "ChannelBridge", "settings.json"), nil
}

func Load(path string) (Config, error) {
	c := Default()
	b, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return c, nil
	}
	if err != nil {
		return c, err
	}
	err = json.Unmarshal(b, &c)
	return c, err
}

// Save replaces preferences atomically, so an interrupted write cannot truncate them.
func Save(path string, c Config) error {
	b, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return err
	}
	if err = os.MkdirAll(filepath.Dir(path), 0700); err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".settings-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(b); err != nil {
		f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), path)
}
