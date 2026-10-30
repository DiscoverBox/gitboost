// Ghidra headless post-script. Locate the same business function without using symbols.
// Usage: analyzeHeadless <project-dir> <project> -import <binary> -scriptPath scripts
//        -postScript AuditRepository.java <output-dir> -deleteProject
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.block.BasicBlockModel;
import ghidra.program.model.block.CodeBlock;
import ghidra.program.model.block.CodeBlockIterator;
import ghidra.program.model.block.CodeBlockReferenceIterator;
import com.google.gson.GsonBuilder;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.*;

public class AuditRepository extends GhidraScript {
    private void collect(Address address, int depth, Set<Address> seen, Set<Function> functions) {
        if (!seen.add(address)) return;
        for (Reference ref : currentProgram.getReferenceManager().getReferencesTo(address)) {
            Function function = getFunctionContaining(ref.getFromAddress());
            if (function != null) functions.add(function);
            else if (depth < 2) collect(ref.getFromAddress(), depth + 1, seen, functions);
        }
    }

    public void run() throws Exception {
        File directory = new File(getScriptArgs()[0]);
        directory.mkdirs();
        String anchor = "仓库地址必须是 github.com/owner/repository";
        byte[] needle = anchor.getBytes(StandardCharsets.UTF_8);
        List<String> anchors = new ArrayList<>();
        Set<Function> functions = new LinkedHashSet<>();
        Address start = currentProgram.getMinAddress();
        Address end = currentProgram.getMaxAddress();
        while (start != null && start.compareTo(end) <= 0) {
            Address found = currentProgram.getMemory().findBytes(start, end, needle, null, true, monitor);
            if (found == null) break;
            anchors.add(found.toString());
            collect(found, 0, new HashSet<>(), functions);
            start = found.next();
        }
        DecompInterface decompiler = new DecompInterface();
        if (!decompiler.openProgram(currentProgram)) throw new IllegalStateException("Cannot open decompiler");
        List<Map<String, Object>> results = new ArrayList<>();
        try {
            for (Function function : functions) {
                Map<String, Object> item = new LinkedHashMap<>();
                item.put("address", function.getEntryPoint().toString());
                item.put("name", function.getName());
                int blocks = 0, edges = 0;
                CodeBlockIterator iterator = new BasicBlockModel(currentProgram)
                    .getCodeBlocksContaining(function.getBody(), monitor);
                while (iterator.hasNext()) {
                    CodeBlock block = iterator.next();
                    blocks++;
                    CodeBlockReferenceIterator destinations = block.getDestinations(monitor);
                    while (destinations.hasNext()) { destinations.next(); edges++; }
                }
                item.put("blocks", blocks);
                item.put("edges", edges);
                DecompileResults result = decompiler.decompileFunction(function, 60, monitor);
                item.put("decompiled", result.decompileCompleted());
                item.put("error", result.getErrorMessage());
                if (result.decompileCompleted()) {
                    String code = result.getDecompiledFunction().getC();
                    Files.writeString(new File(directory, function.getEntryPoint() + ".c").toPath(), code);
                    item.put("decompilerChars", code.length());
                }
                results.add(item);
            }
        } finally {
            decompiler.dispose();
        }
        Map<String, Object> report = new LinkedHashMap<>();
        report.put("input", currentProgram.getExecutablePath());
        report.put("anchor", anchor);
        report.put("anchors", anchors);
        report.put("functions", results);
        Files.writeString(new File(directory, "report.json").toPath(),
            new GsonBuilder().setPrettyPrinting().disableHtmlEscaping().create().toJson(report));
        if (results.isEmpty()) throw new IllegalStateException("No function found from error-string references");
        for (Map<String, Object> item : results) {
            if (!Boolean.TRUE.equals(item.get("decompiled"))) throw new IllegalStateException("Decompilation failed");
        }
        println("Repository audit: " + results);
    }
}
