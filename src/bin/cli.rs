// CLI 客户端入口 -- 完整实现在后续任务逐步填充。
//
// `cli` 模块以文件目录形式放在 src/cli/,通过相对路径声明,
// 不挂载到 lib.rs,避免与后端集成测试互相干扰。

#[path = "../cli/mod.rs"]
mod cli;

fn main() {
    println!("cli bin placeholder");
}
