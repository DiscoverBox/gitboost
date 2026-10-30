// Identical analysis options for both single-function experiment variants.
import ghidra.app.script.GhidraScript;
public class PrepareOllvmAudit extends GhidraScript {
    public void run() throws Exception {
        for (String option : new String[] { "Decompiler Switch Analysis", "Non-Returning Functions - Discovered" }) {
            if (!getCurrentAnalysisOptionsAndValues(currentProgram).containsKey(option))
                throw new IllegalStateException("Missing analysis option: " + option);
            setAnalysisOption(currentProgram, option, "false");
        }
        println("Disabled whole-program switch recovery and discovered non-returning functions");
    }
}
