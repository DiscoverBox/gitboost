#include "llvm/IR/Module.h"
#include "llvm/IR/Verifier.h"
#include "llvm/Passes/PassBuilder.h"
#include "llvm/Plugins/PassPlugin.h"
#include "llvm/Support/ErrorHandling.h"
#include "llvm/Support/raw_ostream.h"
#include "bogus/BogusControlFlow.h"
#include "substitution/Substitution.h"
#include "utils/CryptoUtils.h"

using namespace llvm;
namespace {
struct GitBoostPass : PassInfoMixin<GitBoostPass> {
  PreservedAnalyses run(Module &M, ModuleAnalysisManager &) {
    for (Function &F : M) {
      if (F.isDeclaration() || !F.getName().contains("_12gitboost_lib8importer24normalize_repository_url") ||
          !F.getName().ends_with("24normalize_repository_url")) continue;
      const auto before = F.size();
      BogusControlFlow(true).runBogusControlFlow(F);
      Substitution(true).runSubstitution(F);
      if (verifyModule(M, &errs())) report_fatal_error("GitBoost obfuscation produced invalid IR");
      errs() << "GITBOOST_OLLVM transformed " << F.getName() << " blocks=" << before << "->" << F.size() << "\n";
      return PreservedAnalyses::none();
    }
    return PreservedAnalyses::all();
  }
};
}
extern "C" LLVM_ATTRIBUTE_WEAK PassPluginLibraryInfo llvmGetPassPluginInfo() {
  cryptoutils->prng_seed("A04252B187478C00A40BC6D81D1A8A52");
  return {LLVM_PLUGIN_API_VERSION, "GitBoostSingleFunction", "1", [](PassBuilder &PB) {
    PB.registerPipelineParsingCallback([](StringRef Name, ModulePassManager &MPM,
                                         ArrayRef<PassBuilder::PipelineElement>) {
      if (Name != "gitboost-ollvm") return false;
      MPM.addPass(GitBoostPass()); return true;
    });
    PB.registerOptimizerLastEPCallback([](ModulePassManager &MPM, OptimizationLevel,
                                         ThinOrFullLTOPhase) { MPM.addPass(GitBoostPass()); });
  }};
}
