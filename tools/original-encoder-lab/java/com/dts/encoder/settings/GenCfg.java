package com.dts.encoder.settings;

import com.dts.encoder.config.ConfigFileBuilder;
import java.io.File;
import org.jdom.Document;
import org.jdom.input.SAXBuilder;

/** GenCfg job.xml outDir name dest stream rate layout bitrate: has the encoder's own code write the .cfg for a job. */
public class GenCfg {
    public static void main(String[] a) throws Exception {
        Model.initialize(58);
        Model.set71LayoutsFiltered(false);
        Document doc = new SAXBuilder().build(new File(a[0]));
        Model.setNewEncoderSettings(Integer.parseInt(a[3]), Integer.parseInt(a[4]), Integer.parseInt(a[5]), Integer.parseInt(a[6]), Integer.parseInt(a[7]));
        boolean ok = ConfigFileBuilder.getInstance().buildConfigFile(a[1], a[2], doc);
        System.out.println(ok ? "ok" : "FAILED");
        System.exit(ok ? 0 : 1);
    }
}
